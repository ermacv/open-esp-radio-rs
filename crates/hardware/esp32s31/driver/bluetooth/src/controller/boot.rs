//! Controller-output and runtime-timer activation after BLE PHY initialization.

#[cfg(target_arch = "riscv32")]
pub(crate) mod connectable_advertising;
#[cfg(target_arch = "riscv32")]
pub(crate) mod peripheral_connection;
#[cfg(target_arch = "riscv32")]
mod scheduler_service;
#[cfg(target_arch = "riscv32")]
pub(crate) use scheduler_service::single_item::SingleItemSchedulerCompletionFaultOwner;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod timed_preparation;
#[cfg(target_arch = "riscv32")]
pub(crate) use scheduler_service::connectable_advertising::{
    LegacyConnectableAdvertisingSchedulerFailStop,
    LegacyConnectableAdvertisingSchedulerFailStopCause,
    LegacyConnectableAdvertisingSchedulerStartRetry,
    LegacyConnectableAdvertisingSchedulerStartRetryError,
    LegacyConnectableAdvertisingSchedulerStartStep,
};

#[cfg(target_arch = "riscv32")]
use crate::{
    ble_phy::ControllerBlePhyEngineInitialized,
    controller::time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimePendingCore,
        ControllerTimePendingCoreStep, ControllerTimePendingOrphanStep, ControllerTimePendingOwner,
        ControllerTimePendingOwnerStep, ControllerTimeRequest, ControllerTimeRequestError,
        drain_controller_time_orphan,
    },
    interrupt::{NrtDefaultInterruptEpoch, PrimaryInterruptStep, PrimaryPublishedInterruptStep},
    le::dtm::post_unlink::{
        DtmPostUnlinkArmError, DtmPostUnlinkMailbox, PostUnlinkRearm, PostUnlinkTake,
    },
    low_power::ControllerRuntimeEndpoints,
    modem_lp_timer_queue::{
        ModemLpTimerEventCell, ModemLpTimerEventPublication, ModemLpTimerExpiration,
        ModemLpTimerExpirationState, ModemLpTimerPublishedInterruptStep, ModemLpTimerSoftwareState,
        ModemLpTimerSoftwareStateStep, ModemLpTimerStableInterruptStep,
    },
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
    },
};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothLowPowerRuntimeControlObservation, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothSchedulerSoftwareListRemovalJoin,
    ControllerHal, InterruptOutputPreparedOwner, InterruptRegistersOwner,
    ModemLpTimerCounterStartedOwner, ModemLpTimerInterruptReadyOwner,
    ModemLpTimerSoftwarePendingOwner,
};

/// Powered Controller after address readiness, IRQ-output preparation and
/// runtime-timer start.
///
/// This state retains the complete BLE PHY epoch, the prepared-but-unrouted
/// interrupt partition and the uniquely started low-power timer. It does not
/// claim stable ISR storage, a CPU route, scheduler activation or operational
/// Link-Layer work.
#[must_use = "the started Bluetooth Controller retains every hardware owner"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerOutputTimerStarted<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) initialized:
        ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _interrupt_output: InterruptOutputPreparedOwner,
    pub(crate) timer: ModemLpTimerCounterStartedOwner,
}

/// Powered Controller with both register owners ready for ISR publication.
///
/// The controller interrupt partition and source-127 timer partition have
/// crossed their final no-MMIO ownership transitions. They remain movable and
/// no CPU route is active; the next platform composition must publish both in
/// stable ISR storage before it enables any of the three routes.
#[must_use = "the prepared Bluetooth interrupt owners must be published before routing"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerInterruptOwnersReady<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized: ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _interrupts: InterruptRegistersOwner,
    _timer: ModemLpTimerInterruptReadyOwner,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

/// Platform boundary that publishes both disjoint owners in stable ISR slots.
///
/// Implementations must either publish both owners atomically and return one
/// affine lease, or return the storage value and both unchanged owners. This
/// transition must not enable a CPU route; routing is a later lifecycle edge.
#[cfg(target_arch = "riscv32")]
pub trait InterruptOwnerStorage: Sized {
    /// Affine proof that both owners remain in the implementation's storage.
    type Published;
    /// Exact pre-publication rejection reason.
    type Error;

    /// Publish both owners without enabling any interrupt source.
    fn publish(
        self,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        Self::Published,
        (
            Self::Error,
            Self,
            InterruptRegistersOwner,
            ModemLpTimerInterruptReadyOwner,
        ),
    >;
}

/// Stable-storage boundary for source-127 task ownership.
///
/// The interrupt platform implements this trait for the same affine lease
/// returned by [`InterruptOwnerStorage`]. Taking an owner leaves the
/// stable ISR slot empty, so repeated interrupt entry cannot touch MMIO while
/// task work owns the timer. Only a fully rearmed owner may be restored.
#[cfg(target_arch = "riscv32")]
pub trait ModemLpTimerSoftwareOwnerStorage {
    /// Exact reason task context could not acquire pending work.
    type TakeError;
    /// Exact reason the fully rearmed owner could not return to ISR storage.
    type RestoreError;

    /// Move software-pending ownership out of stable ISR storage.
    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Self::TakeError>;

    /// Restore only a fully rearmed owner, retaining it on rejection.
    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, ModemLpTimerInterruptReadyOwner)>;
}

/// Stable platform dispatch over the published source-127 interrupt owner.
///
/// Implementations retain either the ready or software-pending affine owner
/// in process-wide storage and perform no executor notification themselves.
#[cfg(target_arch = "riscv32")]
pub trait ModemLpTimerInterruptDispatchStorage {
    /// Exact reason the stable owner could not service this entry.
    type Error;

    /// Execute one finite register entry and return its semantic disposition.
    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerStableInterruptStep, Self::Error>;
}

/// Stable platform dispatch over the published shared interrupt owner.
///
/// Implementations must retain the unique primary/NRT register owner in
/// stable storage across every call. Both methods execute exactly one finite
/// Controller disposition and enable no CPU route themselves.
#[cfg(target_arch = "riscv32")]
pub trait SharedInterruptDispatchStorage {
    /// Exact reason the shared owner could not service an entry.
    type Error;

    /// Capture, acknowledge and classify one primary source-124 epoch.
    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptStep, Self::Error>;

    /// Capture and acknowledge one default-profile NRT source-133 epoch.
    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error>;
}

/// Stable task-side access to the shared interrupt owner for scheduler start.
///
/// This is deliberately separate from hard-handler dispatch. Implementations
/// must execute the finite dynamic interrupt preparation synchronously while
/// retaining the owner in the same stable slot. The Controller state that
/// calls it guarantees that CPU routes are still inactive.
#[cfg(target_arch = "riscv32")]
pub trait SchedulerRunInterruptStorage {
    /// Exact reason the stable owner could not prepare scheduler interrupts.
    type Error;

    /// Clear stale dynamic sources and enable the scheduler-run groups.
    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error>;

    /// Recheck the complete scheduler software-list removal predicate while
    /// the stable interrupt-register owner remains in platform storage.
    ///
    /// Failure returns the unchanged affine empty-head proof.
    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut ControllerHal<'_>,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Result<
        BluetoothSchedulerSoftwareListRemovalJoin,
        BluetoothSchedulerHardwareListHeadEmptyObserved,
    >;
}

/// Failed DTM scheduler start before the synchronous run suffix began.
///
/// The published head is returned unchanged. Once interrupt preparation
/// succeeds, every remaining operation is infallible and ownership advances
/// directly to [`crate::scheduler::DtmSchedulerRunning`].
#[must_use = "a failed DTM scheduler start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct DtmSchedulerStartFailure<Role, E> {
    error: E,
    head: crate::scheduler::DtmSchedulerHeadPublished<Role>,
}

/// Failed advertising scheduler start before the synchronous run suffix began.
#[must_use = "a failed advertising scheduler start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct LegacyAdvertisingSchedulerStartFailure<'a, E> {
    error: E,
    head: crate::scheduler::LegacyAdvertisingSchedulerHeadPublished<'a>,
}

/// Failed scanner scheduler start before the synchronous RUN suffix began.
#[must_use = "a failed scanner start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct PassiveScanSchedulerStartFailure<E> {
    error: E,
    head: crate::scheduler::PassiveScanSchedulerHeadPublished,
}

/// Failed connection scheduler start before the synchronous RUN suffix began.
#[must_use = "a failed connection start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct PeripheralConnectionSchedulerStartFailure<E> {
    error: E,
    head: crate::scheduler::PeripheralConnectionSchedulerHeadPublished,
}

#[cfg(target_arch = "riscv32")]
impl<E> PeripheralConnectionSchedulerStartFailure<E> {
    /// Inspect the stable interrupt-storage rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged published connection graph.
    pub fn into_parts(
        self,
    ) -> (
        E,
        crate::scheduler::PeripheralConnectionSchedulerHeadPublished,
    ) {
        (self.error, self.head)
    }
}

#[cfg(target_arch = "riscv32")]
impl<E> PassiveScanSchedulerStartFailure<E> {
    /// Inspect the exact stable interrupt-storage rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged published scanner graph.
    pub fn into_parts(self) -> (E, crate::scheduler::PassiveScanSchedulerHeadPublished) {
        (self.error, self.head)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'a, E> LegacyAdvertisingSchedulerStartFailure<'a, E> {
    pub const fn error(&self) -> &E {
        &self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        E,
        crate::scheduler::LegacyAdvertisingSchedulerHeadPublished<'a>,
    ) {
        (self.error, self.head)
    }
}

#[cfg(target_arch = "riscv32")]
impl<Role, E> DtmSchedulerStartFailure<Role, E> {
    /// Inspect the exact stable-storage rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged published DTM head.
    pub fn into_parts(self) -> (E, crate::scheduler::DtmSchedulerHeadPublished<Role>) {
        (self.error, self.head)
    }
}

/// Controller result of consuming one opaque post-unlink event pair.
#[must_use = "every outcome retains the exact unlinked or removal-ready graph"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSoftwareListRemovalPublishedStep<Role> {
    /// The supplied owner belongs to another mailbox identity or generation.
    MailboxAffinityMismatch(
        crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
    ),
    /// The primary epoch reported a baseline or unclassified fault.
    Fault {
        /// Already-unlinked graph retained for fail-stop handling.
        unlinked: crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact primary controller fault.
        fault: crate::interrupt::PrimaryControllerFault,
    },
    /// The acknowledged epoch contained no reviewed scheduler work.
    NoSchedulerWork {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
        /// Exact acknowledged empty primary epoch.
        epoch: crate::interrupt::PrimaryNoSchedulerWork,
    },
    /// An interrupt-derived scheduler observation was not ready.
    PublishedPending {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
    },
    /// A direct task-side scheduler observation was not ready.
    DirectPending {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
    },
    /// The stable interrupt-register owner was unavailable for direct recheck.
    RecheckUnavailable {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
    },
    /// An internal mailbox invariant rejected re-arm after an empty primary epoch.
    NoSchedulerWorkRearmMismatch {
        unlinked: crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        epoch: crate::interrupt::PrimaryNoSchedulerWork,
    },
    /// An internal mailbox invariant rejected re-arm after a pending scheduler gate.
    PendingRearmMismatch {
        unlinked: crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
    },
    /// An internal mailbox invariant rejected re-arm after direct recheck.
    RecheckRearmMismatch {
        unlinked: crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
    },
    /// The graph belongs to another Controller scheduler epoch.
    SchedulerIdentityMismatch {
        /// Unchanged already-unlinked graph.
        unlinked: crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact classified event which was not consumed by the mismatched graph.
        event: crate::interrupt::PrimarySchedulerEvent,
    },
    /// The graph does not belong to the scheduler epoch performing a direct recheck.
    DirectSchedulerIdentityMismatch {
        unlinked: crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
    },
    /// The complete post-unlink return predicate became ready.
    Ready {
        /// Exact removal-ready graph; CPU ownership is still not returned.
        ready: crate::scheduler::DtmSchedulerSoftwareListRemovalReady<Role>,
    },
}

/// Result of atomically unlinking one completed DTM item and arming the
/// Controller-owned post-unlink mailbox.
#[must_use = "retain the empty-head graph or the armed post-unlink owner"]
#[cfg(target_arch = "riscv32")]
pub enum DtmPostUnlinkArmStep<Role> {
    MailboxBusy(crate::scheduler::DtmSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxIdentityExhausted(crate::scheduler::DtmSchedulerHardwareHeadEmptyObserved<Role>),
    GenerationExhausted(crate::scheduler::DtmSchedulerHardwareHeadEmptyObserved<Role>),
    SchedulerIdentityMismatch(crate::scheduler::DtmSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxCommitMismatch(crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>),
    Armed(
        crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
    ),
}

/// Stable state retained inside the disjoint source-127 task endpoint.
#[cfg(target_arch = "riscv32")]
enum ControllerModemTimerTaskPhase {
    Idle,
    Work(ModemLpTimerSoftwareState),
    Expiration(ModemLpTimerExpirationState),
    Rearm(ModemLpTimerInterruptReadyOwner),
}

/// Borrowed readiness class for the source-127 task endpoint.
///
/// The value carries no timer owner and may be held by an executor wait future.
/// The affine queue, epoch and HAL phase remain inside
/// [`ControllerModemTimerTask`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum ControllerModemTimerReadinessClass {
    /// No interrupt-owned software work is known to be pending.
    Interrupt,
    /// One finite software transition can run immediately.
    Step,
    /// Expiration publication is waiting for the one-event cell to become empty.
    EventCapacity,
    /// A fully rearmed owner is ready for stable-storage restoration.
    Rearm,
}

/// Owner-free borrowed readiness observation for source 127.
#[must_use = "readiness must be checked before polling again"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerModemTimerReadiness<'task> {
    class: ControllerModemTimerReadinessClass,
    worker_wake: &'task crate::modem_lp_timer_queue::ModemLpTimerWorkerWakeCell,
    events: &'task ModemLpTimerEventCell,
}

#[cfg(target_arch = "riscv32")]
impl ControllerModemTimerReadiness<'_> {
    /// Exact state an executor wait is observing.
    pub const fn class(&self) -> ControllerModemTimerReadinessClass {
        self.class
    }

    /// Recheck readiness after registering the executor's own waker.
    ///
    /// This operation is borrowed and value-only. It neither acquires stable
    /// ownership nor advances queue or register state.
    pub fn is_ready(&self) -> bool {
        match self.class {
            ControllerModemTimerReadinessClass::Interrupt => self.worker_wake.is_pending(),
            ControllerModemTimerReadinessClass::Step
            | ControllerModemTimerReadinessClass::Rearm => true,
            ControllerModemTimerReadinessClass::EventCapacity => !self.events.is_pending(),
        }
    }
}

/// Result of acquiring one durable source-127 software-pending owner.
#[must_use = "a begin result must be handled without losing stable readiness"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerModemTimerBegin<E> {
    /// No durable interrupt publication currently requests task work.
    NotReady,
    /// The owner entered the endpoint and one finite software step is ready.
    Started,
    /// Stable storage rejected acquisition; the endpoint remains idle.
    StorageRejected(E),
    /// A software or rearm phase is already retained by this endpoint.
    AlreadyActive,
}

/// Result of one finite source-127 task transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the next source-127 readiness class must be observed"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerModemTimerStep {
    /// No software owner has been acquired.
    Idle,
    /// One expiration now owns the durable publication edge.
    ExpirationPending(ModemLpTimerExpiration),
    /// The expiration was published and later work remains retained.
    Published(ModemLpTimerEventPublication),
    /// The expiration cell is occupied and the unchanged publication remains retained.
    Backpressured(ModemLpTimerExpiration),
    /// Immediate compare disposition requires a later fresh finite recheck.
    Recheck,
    /// Queue processing produced a fully rearmed owner retained inside the endpoint.
    RearmPending,
}

/// Result of restoring the fully rearmed owner to stable ISR storage.
#[must_use = "a rejected rearm remains retained by the timer task"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerModemTimerRearm<E> {
    /// The ready owner is back in stable interrupt storage.
    Rearmed,
    /// Stable storage rejected restoration; the exact ready owner remains private.
    StorageRejected(E),
    /// No fully rearmed owner is currently retained.
    NotReady,
}

/// Disjoint task-context source-127 owner for one final Controller runtime.
///
/// This endpoint exclusively owns the mutable timer queue and positional epoch.
/// Stable storage is shared only as the platform exchange boundary with the
/// interrupt service. All affine HAL states remain private across borrowed
/// readiness and finite `begin`, `step`, and `rearm` calls, so an executor
/// future never needs to own them.
#[must_use = "the modem timer task retains source-127 queue and hardware ownership"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerModemTimerTask<'runtime, S, const CAPACITY: usize> {
    storage: &'runtime S,
    runtime: ControllerModemTimerRuntime<'runtime, CAPACITY>,
    phase: ControllerModemTimerTaskPhase,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize> ControllerModemTimerTask<'runtime, S, CAPACITY>
where
    S: ModemLpTimerSoftwareOwnerStorage,
{
    fn new(storage: &'runtime S, runtime: ControllerModemTimerRuntime<'runtime, CAPACITY>) -> Self {
        Self {
            storage,
            runtime,
            phase: ControllerModemTimerTaskPhase::Idle,
        }
    }

    /// Borrow the current owner-free readiness predicate.
    pub fn readiness(&self) -> ControllerModemTimerReadiness<'_> {
        let class = match &self.phase {
            ControllerModemTimerTaskPhase::Idle => ControllerModemTimerReadinessClass::Interrupt,
            ControllerModemTimerTaskPhase::Work(_) => ControllerModemTimerReadinessClass::Step,
            ControllerModemTimerTaskPhase::Expiration(_) => {
                ControllerModemTimerReadinessClass::EventCapacity
            }
            ControllerModemTimerTaskPhase::Rearm(_) => ControllerModemTimerReadinessClass::Rearm,
        };
        ControllerModemTimerReadiness {
            class,
            worker_wake: self.runtime.worker_wake(),
            events: self.runtime.events(),
        }
    }

    /// Acquire exactly one software-pending owner after borrowed readiness.
    pub fn begin(&mut self) -> ControllerModemTimerBegin<S::TakeError> {
        if !matches!(self.phase, ControllerModemTimerTaskPhase::Idle) {
            return ControllerModemTimerBegin::AlreadyActive;
        }
        if !self.runtime.worker_wake().is_pending() {
            return ControllerModemTimerBegin::NotReady;
        }
        match self.storage.take_modem_lp_timer_software_pending() {
            Ok(owner) => {
                self.runtime.worker_wake.take();
                self.phase = ControllerModemTimerTaskPhase::Work(ModemLpTimerSoftwareState::begin(
                    owner,
                    self.runtime.epoch,
                ));
                ControllerModemTimerBegin::Started
            }
            Err(error) => ControllerModemTimerBegin::StorageRejected(error),
        }
    }

    /// Advance exactly one queue, publication or compare transition.
    pub fn step(&mut self) -> ControllerModemTimerStep {
        let phase = core::mem::replace(&mut self.phase, ControllerModemTimerTaskPhase::Idle);
        match phase {
            ControllerModemTimerTaskPhase::Idle => ControllerModemTimerStep::Idle,
            ControllerModemTimerTaskPhase::Work(work) => {
                match work.step(self.runtime.queue, self.runtime.epoch) {
                    ModemLpTimerSoftwareStateStep::Expiration(pending) => {
                        let event = pending.event();
                        self.phase = ControllerModemTimerTaskPhase::Expiration(pending);
                        ControllerModemTimerStep::ExpirationPending(event)
                    }
                    ModemLpTimerSoftwareStateStep::Recheck(work) => {
                        self.phase = ControllerModemTimerTaskPhase::Work(work);
                        ControllerModemTimerStep::Recheck
                    }
                    ModemLpTimerSoftwareStateStep::Rearmed(owner) => {
                        self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
                        ControllerModemTimerStep::RearmPending
                    }
                }
            }
            ControllerModemTimerTaskPhase::Expiration(pending) => {
                let event = pending.event();
                match pending.publish(self.runtime.events()) {
                    Ok((work, publication)) => {
                        self.phase = ControllerModemTimerTaskPhase::Work(work);
                        ControllerModemTimerStep::Published(publication)
                    }
                    Err(pending) => {
                        self.phase = ControllerModemTimerTaskPhase::Expiration(pending);
                        ControllerModemTimerStep::Backpressured(event)
                    }
                }
            }
            ControllerModemTimerTaskPhase::Rearm(owner) => {
                self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
                ControllerModemTimerStep::RearmPending
            }
        }
    }

    /// Restore one fully rearmed owner to stable source-127 interrupt storage.
    pub fn rearm(&mut self) -> ControllerModemTimerRearm<S::RestoreError> {
        let phase = core::mem::replace(&mut self.phase, ControllerModemTimerTaskPhase::Idle);
        let ControllerModemTimerTaskPhase::Rearm(owner) = phase else {
            self.phase = phase;
            return ControllerModemTimerRearm::NotReady;
        };
        self.runtime.worker_wake.take();
        match self.storage.restore_modem_lp_timer_ready(owner) {
            Ok(()) => ControllerModemTimerRearm::Rearmed,
            Err((error, owner)) => {
                self.phase = ControllerModemTimerTaskPhase::Rearm(owner);
                ControllerModemTimerRearm::StorageRejected(error)
            }
        }
    }

    /// Consume one durably published expiration, if present.
    pub fn take_expiration(&mut self) -> Option<ModemLpTimerExpiration> {
        self.runtime.events().take()
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
pub struct ControllerInterruptOwnersPublished<
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized: ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _storage: S,
    post_unlink_mailbox: DtmPostUnlinkMailbox,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
    scheduler_epoch: Option<crate::ControllerSchedulerEpoch>,
    dtm_resources: crate::le::dtm::DtmRuntimeResources,
    legacy_advertising_resources: crate::le::advertising::LegacyAdvertisingRuntimeResources,
    passive_scan_resources: crate::le::scanning::PassiveScanRuntimeResources,
    peripheral_connection_resources: crate::le::peripheral::PeripheralConnectionRuntimeResources,
    legacy_connectable_advertising_resources:
        crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
}

/// Hardware/task endpoints prepared after stable interrupt-owner publication.
///
/// HCI is intentionally absent: protocol resources are bound only after this
/// hardware ownership graph has reached its final movable state.
#[must_use = "published hardware endpoints must remain in one live runtime epoch"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct ControllerPublishedHardwareRuntimeEndpoints<
    'runtime,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) interrupt: ControllerPublishedInterruptService<'runtime, S>,
    pub(crate) task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    pub(crate) modem_timer: ControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedHardwareRuntimeEndpoints<
        'runtime,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
{
    pub(crate) fn bind_hci<M, const H2C: usize, const C2H: usize, const PC: usize>(
        self,
        mut hci: oer_bluetooth_hci::LeControllerHciEndpoints<'runtime, M, H2C, C2H, PC>,
    ) -> ControllerPublishedRuntimeSplit<
        'runtime,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        H2C,
        C2H,
        PC,
    >
    where
        M: RawMutex,
    {
        let Self {
            interrupt,
            task,
            modem_timer,
        } = self;
        match hci.controller.claim_initial_command_ready(task) {
            oer_bluetooth_hci::LeControllerCommandReadyClaim::Ready(ready) => {
                ControllerPublishedRuntimeSplit::Ready(ControllerPublishedRuntimeEndpoints {
                    interrupt,
                    task: ControllerIdleCommandTask::from_ready(ready),
                    modem_timer,
                    hci,
                })
            }
            oer_bluetooth_hci::LeControllerCommandReadyClaim::AlreadyClaimed(task) => {
                ControllerPublishedRuntimeSplit::CommandReadyUnavailable(
                    ControllerPublishedRuntimeSplitFailure {
                        _interrupt: interrupt,
                        _task: task,
                        _modem_timer: modem_timer,
                        _hci: hci,
                    },
                )
            }
        }
    }
}

/// Disjoint runtime endpoints borrowed from one statically placed final
/// Controller owner.
///
/// The task endpoint owns all mutable scheduler workers, the interrupt service
/// owns only stable platform dispatch plus shared publication cells, and HCI
/// exposes the Host transport and combined Controller command endpoint. Keeping
/// the backing final owner in caller-owned stable storage prevents a
/// self-referential runtime object.
#[must_use = "the final Controller endpoints must remain in one live runtime epoch"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerPublishedRuntimeEndpoints<
    'runtime,
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
    /// Finite hard-handler service over the stable PAC/HAL owners.
    pub interrupt: ControllerPublishedInterruptService<'runtime, S>,
    /// Sole idle task carrying this epoch's affine next-command authority.
    pub task: ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
    /// Disjoint source-127 queue, epoch and stable-storage task endpoint.
    pub modem_timer: ControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
    /// Disjoint Host transport and combined Controller command endpoint.
    pub hci: oer_bluetooth_hci::LeControllerHciEndpoints<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Result of borrowing one final Controller runtime epoch.
///
/// A successful split claims the HCI epoch's initial command-ready authority
/// exactly once. Every later split fails closed without releasing the powered
/// task, interrupt service, or combined HCI endpoint as independently usable
/// values.
#[must_use = "retain the ready runtime or its opaque fail-stop owner"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerPublishedRuntimeSplit<
    'runtime,
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
    /// The only idle command task for this Controller epoch was claimed.
    Ready(
        ControllerPublishedRuntimeEndpoints<
            'runtime,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ),
    /// The epoch's initial command authority had already been consumed.
    CommandReadyUnavailable(
        ControllerPublishedRuntimeSplitFailure<
            'runtime,
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

/// Opaque fail-stop owner for a final runtime whose command authority was gone.
#[must_use = "the complete unavailable runtime remains intentionally fail-stopped"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerPublishedRuntimeSplitFailure<
    'runtime,
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
    _interrupt: ControllerPublishedInterruptService<'runtime, S>,
    _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _modem_timer: ControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
    _hci: oer_bluetooth_hci::LeControllerHciEndpoints<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Idle powered task paired with the sole affine next-command authority.
///
/// The raw task service and unit order token are deliberately private. Only a
/// validated full-classification route may consume this aggregate.
#[must_use = "route the next command without separating task and HCI order"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerIdleCommandTask<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    ready: oer_bluetooth_hci::LeControllerCommandReady<'runtime, ()>,
}

/// One non-blocking idle command intake through the combined HCI endpoint.
///
/// Every non-command branch returns the complete idle task, including its sole
/// affine next-command authority. A consumed command is routed immediately;
/// neither its classification nor its order token is exposed separately.
#[must_use = "route the command or retain the returned idle task"]
#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc intake variants retain the complete affine Controller owner"
)]
pub enum ControllerIdleCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    /// One command was consumed and routed; scratch storage is reusable.
    Routed {
        route:
            crate::le::dtm::ControllerIdleCommandRoute<'runtime, 'command, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    /// A readiness hint became stale before intake.
    Empty {
        task: ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    /// The supplied combined endpoint belongs to another HCI epoch.
    EndpointMismatch {
        task: ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    /// Packet intake failed without consuming next-command authority.
    Channel {
        task: ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
        error: oer_bluetooth_hci::HciChannelError,
    },
    /// The oldest Host packet was data rather than a Controller command.
    NonCommand {
        task: ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        frame: oer_bluetooth_hci::HciEpochBound<
            'command,
            oer_bluetooth_hci::HostToControllerFrame<'buffer>,
        >,
    },
}

/// One idle Controller response retaining the powered task until publication.
#[must_use = "publish the response or retain the unchanged idle transaction"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerIdleResponsePending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    transaction: oer_bluetooth_hci::LeControllerResponsePending<
        'runtime,
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Result of one idle response publication attempt.
#[must_use = "retain backpressure, mismatch, fault, or the returned idle command task"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerIdleResponsePublication<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// The response was durably inserted and next-command authority returned.
    Published(ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// Controller-to-Host capacity was unavailable.
    Pending(ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The supplied endpoint belongs to another HCI epoch.
    EndpointMismatch(ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// A non-capacity fault retained the exact response transaction.
    Fault {
        pending: ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        error: oer_bluetooth_hci::HciChannelError,
    },
}

/// Idle Reset retaining both the task and exact command/order authority.
#[must_use = "complete Reset only through the matching combined endpoint"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerIdleResetBarrier<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    barrier: oer_bluetooth_hci::LeControllerResetBarrier<
        'runtime,
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Result of applying one already-idle Reset.
#[must_use = "publish the Reset response or retain the endpoint mismatch"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerIdleResetCompletion<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Reset was applied exactly once and its response awaits publication.
    ResponsePending(ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The endpoint belongs to another HCI epoch; Reset remains unapplied.
    EndpointMismatch(ControllerIdleResetBarrier<'runtime, S, SCHEDULER_CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn new(
        transaction: oer_bluetooth_hci::LeControllerResponsePending<
            'runtime,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> Self {
        Self { transaction }
    }

    /// Whether the exact pending response belongs to this endpoint.
    pub fn matches_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.transaction.matches_endpoint(controller)
    }

    /// Wait until the matching Controller-to-Host queue may accept this response.
    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    /// Attempt exact-once publication through the matching endpoint.
    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> ControllerIdleResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        match self.transaction.try_publish(controller) {
            oer_bluetooth_hci::LeControllerResponsePublication::Published(ready) => {
                ControllerIdleResponsePublication::Published(ControllerIdleCommandTask::from_ready(
                    ready,
                ))
            }
            oer_bluetooth_hci::LeControllerResponsePublication::Pending(transaction) => {
                ControllerIdleResponsePublication::Pending(Self { transaction })
            }
            oer_bluetooth_hci::LeControllerResponsePublication::EndpointMismatch(transaction) => {
                ControllerIdleResponsePublication::EndpointMismatch(Self { transaction })
            }
            oer_bluetooth_hci::LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => ControllerIdleResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerIdleResetBarrier<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn new(
        barrier: oer_bluetooth_hci::LeControllerResetBarrier<
            'runtime,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> Self {
        Self { barrier }
    }

    /// Apply Reset only after the chip's idle aggregate proves quiescence.
    pub fn complete<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> ControllerIdleResetCompletion<'runtime, S, SCHEDULER_CAPACITY> {
        match controller.complete_reset_after_quiescence(self.barrier) {
            oer_bluetooth_hci::LeControllerResetCompletion::ResponsePending(transaction) => {
                ControllerIdleResetCompletion::ResponsePending(ControllerIdleResponsePending::new(
                    transaction,
                ))
            }
            oer_bluetooth_hci::LeControllerResetCompletion::EndpointMismatch(barrier) => {
                ControllerIdleResetCompletion::EndpointMismatch(Self { barrier })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) fn from_ready(
        ready: oer_bluetooth_hci::LeControllerCommandReady<
            'runtime,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> Self {
        let (task, ready) = ready.into_parts();
        Self { task, ready }
    }

    pub(crate) const fn from_parts(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        ready: oer_bluetooth_hci::LeControllerCommandReady<'runtime, ()>,
    ) -> Self {
        Self { task, ready }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        oer_bluetooth_hci::LeControllerCommandReady<'runtime, ()>,
    ) {
        (self.task, self.ready)
    }

    pub(crate) fn into_ready(
        self,
    ) -> oer_bluetooth_hci::LeControllerCommandReady<
        'runtime,
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        self.ready.map_owner(|()| self.task)
    }

    /// Whether this idle task belongs to the supplied Controller endpoint.
    pub fn accepts_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.ready.accepts_endpoint(controller)
    }

    /// Wait until the matching Host queue may contain a command.
    ///
    /// The wait only borrows the idle aggregate, so cancellation cannot lose
    /// task ownership or affine next-command authority.
    pub async fn wait_command_available<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_command_available(&self.ready).await
    }

    /// Consume, classify and route at most one Host command while idle.
    ///
    /// RX/TX starts retain their portable deferred response through the entire
    /// first-event runner. Every other routed branch retains either the exact
    /// pending response or Reset barrier. No classification can escape without
    /// its affine command authority.
    pub fn try_route_idle_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<
            'command,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        buffer: &'buffer mut [u8],
    ) -> ControllerIdleCommandIntake<'runtime, 'command, 'buffer, S, SCHEDULER_CAPACITY>
    where
        S: SchedulerRunInterruptStorage,
    {
        match controller.try_receive_classified_command_with_buffer(self.into_ready(), buffer) {
            oer_bluetooth_hci::LeControllerCommandIntake::Command { command, buffer } => {
                ControllerIdleCommandIntake::Routed {
                    route: Self::route_idle_classified_command(controller, command),
                    buffer,
                }
            }
            oer_bluetooth_hci::LeControllerCommandIntake::Empty { ready, buffer } => {
                ControllerIdleCommandIntake::Empty {
                    task: Self::from_ready(ready),
                    buffer,
                }
            }
            oer_bluetooth_hci::LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                ControllerIdleCommandIntake::EndpointMismatch {
                    task: Self::from_ready(ready),
                    buffer,
                }
            }
            oer_bluetooth_hci::LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => ControllerIdleCommandIntake::Channel {
                task: Self::from_ready(ready),
                buffer,
                error,
            },
            oer_bluetooth_hci::LeControllerCommandIntake::NonCommand { ready, frame } => {
                ControllerIdleCommandIntake::NonCommand {
                    task: Self::from_ready(ready),
                    frame,
                }
            }
        }
    }

    fn route_idle_classified_command<
        'command,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<
            'command,
            M,
            H2C,
            C2H,
            PACKET,
        >,
        command: oer_bluetooth_hci::LeControllerClassifiedCommand<
            'runtime,
            'command,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> crate::le::dtm::ControllerIdleCommandRoute<'runtime, 'command, S, SCHEDULER_CAPACITY>
    where
        S: SchedulerRunInterruptStorage,
    {
        match controller.route_idle_classified_command(command) {
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartReceiver(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::le::dtm::DtmFirstRunner::begin(
                    task,
                    crate::le::dtm::DtmDeferredStart::receiver(deferred),
                ) {
                    Ok(runner) => crate::le::dtm::ControllerIdleCommandRoute::Start(runner),
                    Err(failure) => crate::le::dtm::ControllerIdleCommandRoute::StartFailed(failure),
                }
            }
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartTransmitter(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::le::dtm::DtmFirstRunner::begin(
                    task,
                    crate::le::dtm::DtmDeferredStart::transmitter(deferred),
                ) {
                    Ok(runner) => crate::le::dtm::ControllerIdleCommandRoute::Start(runner),
                    Err(failure) => crate::le::dtm::ControllerIdleCommandRoute::StartFailed(failure),
                }
            }
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartLegacyNonconnectableAdvertising(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::le::advertising::LegacyAdvertisingFirstRunner::begin(task, deferred) {
                    Ok(runner) => {
                        crate::le::dtm::ControllerIdleCommandRoute::StartLegacyNonconnectableAdvertising(runner)
                    }
                    Err(failure) => {
                        crate::le::dtm::ControllerIdleCommandRoute::LegacyAdvertisingStartFailed(failure)
                    }
                }
            }
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartLegacyConnectableAdvertising(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::le::advertising::LegacyConnectableAdvertisingFirstRunner::begin(
                    task, deferred,
                ) {
                    Ok(runner) => {
                        crate::le::dtm::ControllerIdleCommandRoute::StartLegacyConnectableAdvertising(
                            runner,
                        )
                    }
                    Err(failure) => {
                        crate::le::dtm::ControllerIdleCommandRoute::LegacyConnectableAdvertisingStartFailed(
                            failure,
                        )
                    }
                }
            }
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartLegacyScanning(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::le::scanning::PassiveScanHciFirstRunner::begin(task, deferred) {
                    Ok(runner) => {
                        crate::le::dtm::ControllerIdleCommandRoute::StartPassiveScanning(runner)
                    }
                    Err(failure) => {
                        crate::le::dtm::ControllerIdleCommandRoute::PassiveScanStartFailed(failure)
                    }
                }
            }
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::ResponsePending(
                pending,
            ) => crate::le::dtm::ControllerIdleCommandRoute::ResponsePending(
                ControllerIdleResponsePending::new(pending),
            ),
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::ResetBarrier(
                barrier,
            ) => crate::le::dtm::ControllerIdleCommandRoute::ResetBarrier(
                ControllerIdleResetBarrier::new(barrier),
            ),
            oer_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::EndpointMismatch(
                command,
            ) => crate::le::dtm::ControllerIdleCommandRoute::EndpointMismatch(
                crate::le::dtm::ControllerIdleCommandMismatch::new(command),
            ),
        }
    }
}

/// Stable interrupt service for a materialized final Controller epoch.
///
/// This value cannot prepare DTM descriptors or mutate task-owned scheduler
/// state. It can only execute the three bounded hardware dispositions and
/// publish their durable events into the matching runtime resources.
#[must_use = "interrupt service must remain paired with its task runtime"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerPublishedInterruptService<'runtime, S> {
    storage: &'runtime S,
    runtime: ControllerInterruptRuntime<'runtime>,
    mailbox: &'runtime DtmPostUnlinkMailbox,
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
pub struct ControllerPublishedTaskService<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    storage: &'runtime S,
    runtime: ControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    mailbox: &'runtime DtmPostUnlinkMailbox,
    dtm_resources: &'runtime mut crate::le::dtm::DtmRuntimeResources,
    legacy_advertising_resources:
        &'runtime mut crate::le::advertising::LegacyAdvertisingRuntimeResources,
    passive_scan_resources: &'runtime mut crate::le::scanning::PassiveScanRuntimeResources,
    peripheral_connection_resources:
        &'runtime mut crate::le::peripheral::PeripheralConnectionRuntimeResources,
    legacy_connectable_advertising_resources:
        &'runtime mut crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
    direction_finding_workspace: oer_esp32s31_bluetooth_memory::DirectionFindingWorkspaceLink,
    ble_phy_timing: crate::ble_phy::BlePhyTimingAuthority,
    scheduler_epoch: &'runtime mut Option<crate::ControllerSchedulerEpoch>,
}

/// Why one completed LE packet cannot yet enter scheduler time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum LePacketStartTimingError {
    /// The mandatory first live controller-time sample has not established the
    /// retained scheduler epoch yet.
    SchedulerEpochUnavailable,
}

/// Why an affine post-enable controller-time acquisition did not start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum AlwaysAwakePostEnableTimeBeginError {
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
impl From<ControllerTimeRequestError> for AlwaysAwakePostEnableTimeBeginError {
    fn from(error: ControllerTimeRequestError) -> Self {
        match error {
            ControllerTimeRequestError::Busy => Self::Busy,
            ControllerTimeRequestError::OwnershipCollision => Self::OwnershipCollision,
            ControllerTimeRequestError::GenerationExhausted => Self::GenerationExhausted,
            ControllerTimeRequestError::Faulted => Self::Faulted,
        }
    }
}

/// Why a fresh scheduler-current acquisition did not start.
///
/// The already-initialized scheduler epoch and exact Controller remain owned
/// by the corresponding [`ControllerSchedulerCurrentBeginFailure`]
/// on every variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum ControllerSchedulerCurrentBeginError {
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
pub struct ControllerSchedulerEpochUnavailable<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerEpochUnavailable<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Recover the unchanged task service.
    pub fn into_task_service(
        self,
    ) -> ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY> {
        self.controller
    }
}

#[cfg(target_arch = "riscv32")]
impl From<ControllerTimeRequestError> for ControllerSchedulerCurrentBeginError {
    fn from(error: ControllerTimeRequestError) -> Self {
        match error {
            ControllerTimeRequestError::Busy => Self::Busy,
            ControllerTimeRequestError::OwnershipCollision => Self::OwnershipCollision,
            ControllerTimeRequestError::GenerationExhausted => Self::GenerationExhausted,
            ControllerTimeRequestError::Faulted => Self::Faulted,
        }
    }
}

/// Fail-stop result of rechecking one affine post-enable time acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum AlwaysAwakePostEnableTimeError {
    /// The affine identity no longer matched the private worker owner.
    RequestMismatch,
    /// The lower sticky owner disappeared while the request was active.
    OwnershipLost,
    /// An earlier ownership mismatch already stopped this worker.
    Faulted,
}

#[cfg(target_arch = "riscv32")]
impl From<ControllerTimeEventError> for AlwaysAwakePostEnableTimeError {
    fn from(error: ControllerTimeEventError) -> Self {
        match error {
            ControllerTimeEventError::RequestMismatch => Self::RequestMismatch,
            ControllerTimeEventError::OwnershipLost => Self::OwnershipLost,
            ControllerTimeEventError::Faulted => Self::Faulted,
        }
    }
}

/// Fail-stop result of rechecking or cancelling a fresh scheduler current.
///
/// Every public failure retains the exact owned scheduler epoch. Faulted
/// ownership never yields a sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum ControllerSchedulerCurrentError {
    /// The affine identity no longer matched the private worker owner.
    RequestMismatch,
    /// The lower sticky owner disappeared while the request was active.
    OwnershipLost,
    /// An earlier ownership mismatch already stopped this worker.
    Faulted,
}

#[cfg(target_arch = "riscv32")]
impl From<ControllerTimeEventError> for ControllerSchedulerCurrentError {
    fn from(error: ControllerTimeEventError) -> Self {
        match error {
            ControllerTimeEventError::RequestMismatch => Self::RequestMismatch,
            ControllerTimeEventError::OwnershipLost => Self::OwnershipLost,
            ControllerTimeEventError::Faulted => Self::Faulted,
        }
    }
}

/// Rejected cold post-enable acquisition retaining the complete task service.
#[must_use = "the task service retained by a rejected acquisition must be handled"]
#[cfg(target_arch = "riscv32")]
pub struct AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    error: AlwaysAwakePostEnableTimeBeginError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact reason the cold acquisition did not start.
    pub const fn error(&self) -> AlwaysAwakePostEnableTimeBeginError {
        self.error
    }

    /// Recover the unchanged task service and exact rejection.
    pub fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        AlwaysAwakePostEnableTimeBeginError,
    ) {
        (self.controller, self.error)
    }
}

/// Failed cold post-enable recheck or cancellation retaining the task service.
#[must_use = "the fail-stop task service must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct AlwaysAwakePostEnableTimeFailure<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    error: AlwaysAwakePostEnableTimeError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact fail-stop observation.
    pub const fn error(&self) -> AlwaysAwakePostEnableTimeError {
        self.error
    }

    /// Recover the task service and exact fail-stop observation.
    pub fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        AlwaysAwakePostEnableTimeError,
    ) {
        (self.controller, self.error)
    }
}

/// Rejected fresh-current acquisition retaining the epoch owner.
#[must_use = "the retained scheduler epoch must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerSchedulerCurrentBeginFailure<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    error: ControllerSchedulerCurrentBeginError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact reason the fresh acquisition did not start.
    pub const fn error(&self) -> ControllerSchedulerCurrentBeginError {
        self.error
    }

    /// Recover the retained epoch and exact rejection.
    pub fn into_parts(
        self,
    ) -> (
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        ControllerSchedulerCurrentBeginError,
    ) {
        (self.controller, self.error)
    }
}

/// Failed fresh-current recheck or cancellation retaining the epoch owner.
#[must_use = "the fail-stop scheduler epoch must remain owned"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerSchedulerCurrentFailure<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    error: ControllerSchedulerCurrentError,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact fail-stop observation.
    pub const fn error(&self) -> ControllerSchedulerCurrentError {
        self.error
    }

    /// Recover the retained epoch and exact fail-stop observation.
    pub fn into_parts(
        self,
    ) -> (
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        ControllerSchedulerCurrentError,
    ) {
        (self.controller, self.error)
    }
}

#[cfg(target_arch = "riscv32")]
const fn controller_time_begin_error(
    error: ControllerTimeRequestError,
) -> crate::scheduler::ControllerTimeAcquisitionError {
    match error {
        ControllerTimeRequestError::Busy => crate::scheduler::ControllerTimeAcquisitionError::Busy,
        ControllerTimeRequestError::OwnershipCollision => {
            crate::scheduler::ControllerTimeAcquisitionError::OwnershipCollision
        }
        ControllerTimeRequestError::GenerationExhausted => {
            crate::scheduler::ControllerTimeAcquisitionError::GenerationExhausted
        }
        ControllerTimeRequestError::Faulted => {
            crate::scheduler::ControllerTimeAcquisitionError::Faulted
        }
    }
}

#[cfg(target_arch = "riscv32")]
const fn controller_time_event_error(
    error: ControllerTimeEventError,
) -> crate::scheduler::ControllerTimeAcquisitionError {
    match error {
        ControllerTimeEventError::RequestMismatch => {
            crate::scheduler::ControllerTimeAcquisitionError::RequestMismatch
        }
        ControllerTimeEventError::OwnershipLost => {
            crate::scheduler::ControllerTimeAcquisitionError::OwnershipLost
        }
        ControllerTimeEventError::Faulted => {
            crate::scheduler::ControllerTimeAcquisitionError::Faulted
        }
    }
}

/// Result of one bounded abandoned-request drain observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a waiting orphan requires one later bounded drain observation"]
#[cfg(target_arch = "riscv32")]
pub enum AlwaysAwakePostEnableTimeOrphanDrainStep {
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
pub enum ControllerTimeOrphanDrainStep {
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
pub struct AlwaysAwakePostEnableTimePending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    core:
        ControllerTimePendingCore<ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>>,
}

/// Controller-bound proof that the post-enable latch request completed.
///
/// The sample remains private and inseparable from the exact owned
/// Controller. This is not an RF-ready instant: this path neither performs nor
/// proves a sleep/wake transition, and it applies no recovered RF-settling
/// interval.
#[must_use = "initialize the persistent scheduler epoch from the bound first-live sample"]
#[cfg(target_arch = "riscv32")]
pub struct AlwaysAwakeTimeObservedAfterEnable<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    sample: crate::ControllerTimeSample,
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
pub struct ControllerSchedulerNowReady<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    epoch: crate::ControllerSchedulerEpoch,
    sample: crate::ControllerTimeSample,
}

/// Exact published Controller after its scheduler epoch has been initialized.
///
/// The epoch remains stored inside the same Controller and cannot detach or be
/// paired with another owner. This state carries no fresh current-time sample;
/// another source-owned observation is required before another preparation.
#[must_use = "the retained scheduler epoch owns the published Controller"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerSchedulerEpochRetained<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
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
pub struct ControllerSchedulerCurrentPending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    core:
        ControllerTimePendingCore<ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>>,
    epoch: crate::ControllerSchedulerEpoch,
}

/// Result of exactly one fresh scheduler-current recheck.
#[must_use = "retain Waiting or consume Ready through one DTM preparation"]
#[cfg(target_arch = "riscv32")]
pub enum ControllerSchedulerCurrentStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Hardware still owns the exact request and complete Controller owner.
    Waiting(ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact request completed with one private epoch-bound sample.
    Ready(ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Finite reason the first legacy-advertising graph returned to idle ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum LegacyAdvertisingControllerPreparationError {
    Set(crate::le::advertising::LegacyAdvertisingSetError),
    Runtime(crate::le::advertising::LegacyAdvertisingRuntimeBeginError),
    LinkState(oer_esp32s31_bluetooth_memory::LegacyAdvertisingPduError),
    TimingWindow,
    Event(crate::scheduler::LegacyAdvertisingFirstEventPreparationError),
    EmptyList(crate::scheduler::SchedulerEmptyListMergeError),
    Cancelled,
}

/// Permanent nonconnectable-advertising preparation fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum LegacyAdvertisingControllerPreparationFailStopCause {
    ControllerTime {
        error: crate::scheduler::ControllerTimeAcquisitionError,
        rollback_failed: bool,
    },
    RuntimeRestore,
    PhaseOwnership,
}

#[cfg(target_arch = "riscv32")]
enum LegacyAdvertisingControllerPreparationFailStopState<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Initial {
        _current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        _cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    },
    Active {
        _controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _cancelled: Option<crate::le::advertising::LegacyAdvertisingCancelled<'static>>,
    },
}

/// Complete owner sealed after controller-time or runtime ownership diverged.
#[must_use = "retain the permanently faulted Controller and advertising graph"]
#[cfg(target_arch = "riscv32")]
pub struct LegacyAdvertisingControllerPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    cause: LegacyAdvertisingControllerPreparationFailStopCause,
    _state: LegacyAdvertisingControllerPreparationFailStopState<'runtime, S, SCHEDULER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    pub const fn cause(&self) -> LegacyAdvertisingControllerPreparationFailStopCause {
        self.cause
    }

    fn active(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: LegacyAdvertisingControllerPreparationFailStopCause,
        cancelled: Option<crate::le::advertising::LegacyAdvertisingCancelled<'static>>,
    ) -> Self {
        Self {
            cause,
            _state: LegacyAdvertisingControllerPreparationFailStopState::Active {
                _controller: controller,
                _cancelled: cancelled,
            },
        }
    }

    fn from_timed(
        failure: timed_preparation::TimedPreparationFailStop<
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
            crate::le::advertising::LegacyAdvertisingCancelled<'static>,
        >,
    ) -> Self {
        let timed_cause = failure.cause();
        let (controller, cancelled) = failure.into_parts();
        let cause = match timed_cause {
            timed_preparation::TimedPreparationFailStopCause::ControllerTime(error) => {
                LegacyAdvertisingControllerPreparationFailStopCause::ControllerTime {
                    error,
                    rollback_failed: cancelled.is_some(),
                }
            }
            timed_preparation::TimedPreparationFailStopCause::Rollback => {
                LegacyAdvertisingControllerPreparationFailStopCause::RuntimeRestore
            }
            timed_preparation::TimedPreparationFailStopCause::PhaseOwnership => {
                LegacyAdvertisingControllerPreparationFailStopCause::PhaseOwnership
            }
        };
        Self::active(controller, cause, cancelled)
    }
}

/// Lossless failure while rebuilding one completed advertising event.
#[must_use = "retain the scheduled or preparation owner for retry or disable"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum LegacyAdvertisingRecurringCandidateFailure {
    SchedulerEpochUnavailable(crate::le::advertising::LegacyAdvertisingNextEventScheduled<'static>),
    Preparation(crate::le::advertising::LegacyAdvertisingRecurringPreparationFailure<'static>),
}

/// Result of applying the recurring sequence sample and empty-list merge.
#[must_use = "retain the task and exact recurring graph outcome together"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum LegacyAdvertisingRecurringSequenceCompletion<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Prepared {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: crate::scheduler::LegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    },
    EventRejected {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::scheduler::LegacyAdvertisingRecurringEventPreparationFailure<'static>,
    },
    EmptyListRejected {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::scheduler::LegacyAdvertisingEmptySchedulerMergeFailure<'static>,
    },
}

/// Terminal result of one source-ordered first advertising preparation.
///
/// Rejection proves that the LL generation and exact SRAM graph were restored
/// to the same task-owned runtime. Success retains the timeline reservation and
/// exclusive-list merge until the later `HEAD` publication edge.
#[must_use = "publish the prepared item or retain the restored task owner"]
#[cfg(target_arch = "riscv32")]
pub enum LegacyAdvertisingControllerPreparationOutcome {
    Prepared(crate::scheduler::LegacyAdvertisingEmptySchedulerMergePrepared<'static>),
    Rejected(LegacyAdvertisingControllerPreparationError),
}

#[cfg(target_arch = "riscv32")]
enum LegacyAdvertisingControllerPreparationPhase {
    AlwaysAwakeTiming {
        reset: crate::le::advertising::LegacyAdvertisingLinkStateReset<'static>,
        now: crate::controller::time::ControllerSchedulerNow,
    },
    Admission(crate::le::advertising::LegacyAdvertisingFirstEventCandidate<'static>),
    Sequence(crate::scheduler::LegacyAdvertisingFirstPreSequence<'static>),
}

/// One exact post-enable timing, admission or sequence-time request.
#[must_use = "recheck or explicitly cancel the exact advertising time request"]
#[cfg(target_arch = "riscv32")]
pub struct LegacyAdvertisingControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    timed: timed_preparation::TimedPreparationPending<
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyAdvertisingControllerPreparationPhase,
        crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    >,
}

/// Terminal advertising preparation with the exact task service retained.
#[must_use = "the task owner and preparation outcome must be handled together"]
#[cfg(target_arch = "riscv32")]
pub struct LegacyAdvertisingControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: LegacyAdvertisingControllerPreparationOutcome,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    pub fn into_parts(
        self,
    ) -> (
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyAdvertisingControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

/// Result of one bounded advertising controller-time observation.
#[must_use = "retain Pending or consume the terminal task and advertising result"]
#[cfg(target_arch = "riscv32")]
pub enum LegacyAdvertisingControllerPreparationStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Pending(LegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>),
    Terminal(LegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum LegacyAdvertisingControllerInitialPreparationFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Rejected {
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        error: LegacyAdvertisingControllerPreparationError,
    },
    FailStop(LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Cancelled advertising preparation awaiting the exact orphan completion.
#[must_use = "drain the abandoned controller-time request before Controller reuse"]
#[cfg(target_arch = "riscv32")]
pub struct LegacyAdvertisingControllerCancellationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    timed: timed_preparation::TimedPreparationCancellationPending<
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

#[must_use = "retain Waiting, recovered Terminal, or sealed FailStop"]
#[cfg(target_arch = "riscv32")]
pub enum LegacyAdvertisingControllerCancellationStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Waiting(LegacyAdvertisingControllerCancellationPending<'runtime, S, SCHEDULER_CAPACITY>),
    Recovered(LegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Finite reason a first passive scanner event returned to idle ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum PassiveScanControllerPreparationError {
    Runtime(crate::le::scanning::PassiveScanRuntimeBeginError),
    TimingWindow,
    Event(crate::scheduler::PassiveScanFirstEventPreparationError),
    EmptyList(crate::scheduler::SchedulerEmptyListMergeError),
    Cancelled,
}

/// Permanent passive-scan preparation fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum PassiveScanControllerPreparationFailStopCause {
    ControllerTime {
        error: crate::scheduler::ControllerTimeAcquisitionError,
        rollback_failed: bool,
    },
    RuntimeRestore,
    PhaseOwnership,
}

#[cfg(target_arch = "riscv32")]
enum PassiveScanControllerPreparationFailStopState<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Active {
        _controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _graph: Option<oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned>,
    },
}

#[must_use = "retain the permanently faulted Controller and scanner graph"]
#[cfg(target_arch = "riscv32")]
pub struct PassiveScanControllerPreparationFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    cause: PassiveScanControllerPreparationFailStopCause,
    _state: PassiveScanControllerPreparationFailStopState<'runtime, S, SCHEDULER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    pub const fn cause(&self) -> PassiveScanControllerPreparationFailStopCause {
        self.cause
    }

    fn active(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: PassiveScanControllerPreparationFailStopCause,
        graph: Option<oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned>,
    ) -> Self {
        Self {
            cause,
            _state: PassiveScanControllerPreparationFailStopState::Active {
                _controller: controller,
                _graph: graph,
            },
        }
    }

    fn from_timed(
        failure: timed_preparation::TimedPreparationFailStop<
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
            oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
        >,
    ) -> Self {
        let timed_cause = failure.cause();
        let (controller, graph) = failure.into_parts();
        let cause = match timed_cause {
            timed_preparation::TimedPreparationFailStopCause::ControllerTime(error) => {
                PassiveScanControllerPreparationFailStopCause::ControllerTime {
                    error,
                    rollback_failed: graph.is_some(),
                }
            }
            timed_preparation::TimedPreparationFailStopCause::Rollback => {
                PassiveScanControllerPreparationFailStopCause::RuntimeRestore
            }
            timed_preparation::TimedPreparationFailStopCause::PhaseOwnership => {
                PassiveScanControllerPreparationFailStopCause::PhaseOwnership
            }
        };
        Self::active(controller, cause, graph)
    }
}

/// Terminal result of one source-ordered first passive scanner preparation.
#[must_use = "publish the scanner item or retain the restored task owner"]
#[cfg(target_arch = "riscv32")]
pub enum PassiveScanControllerPreparationOutcome {
    Prepared {
        merged: crate::scheduler::PassiveScanEmptySchedulerMergePrepared,
        phase: crate::le::scanning::PassiveScanEventPhase,
    },
    Rejected(PassiveScanControllerPreparationError),
}

#[cfg(target_arch = "riscv32")]
enum PassiveScanControllerPreparationPhase {
    AlwaysAwakeTiming {
        graph: oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
        channel: oer_esp32s31_bluetooth_memory::PassiveScanPrimaryChannel,
        parameters: oer_bluetooth_ll::scanning::LegacyPassiveScanParameters,
        previous_phase: Option<crate::le::scanning::PassiveScanEventPhase>,
        now: crate::controller::time::ControllerSchedulerNow,
    },
    Admission {
        candidate: crate::scheduler::PassiveScanFirstEventCandidate,
        phase: crate::le::scanning::PassiveScanEventPhase,
    },
    Sequence {
        admitted: crate::scheduler::PassiveScanFirstPreSequence,
        phase: crate::le::scanning::PassiveScanEventPhase,
    },
}

/// One exact post-enable timing, admission or sequence-time request.
#[must_use = "recheck or explicitly cancel the exact scanner time request"]
#[cfg(target_arch = "riscv32")]
pub struct PassiveScanControllerPreparationPending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    timed: timed_preparation::TimedPreparationPending<
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        PassiveScanControllerPreparationPhase,
        oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
    >,
}

/// Terminal scanner preparation with the exact task service retained.
#[must_use = "the task owner and scanner preparation outcome must be handled together"]
#[cfg(target_arch = "riscv32")]
pub struct PassiveScanControllerPreparationTerminal<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: PassiveScanControllerPreparationOutcome,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    pub fn into_parts(
        self,
    ) -> (
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        PassiveScanControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

/// Result of one bounded passive scanner controller-time observation.
#[must_use = "retain Pending or consume the terminal task and scanner result"]
#[cfg(target_arch = "riscv32")]
pub enum PassiveScanControllerPreparationStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Pending(PassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>),
    Terminal(PassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Lossless rejection before or during a first scanner preparation.
#[must_use = "retain the source-owned current or terminal scanner transaction"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum PassiveScanControllerInitialPreparationFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Rejected {
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        error: PassiveScanControllerPreparationError,
    },
    FailStop(PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

#[must_use = "drain the abandoned scanner-time request before Controller reuse"]
#[cfg(target_arch = "riscv32")]
pub struct PassiveScanControllerCancellationPending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    timed: timed_preparation::TimedPreparationCancellationPending<
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

#[must_use = "retain Waiting, recovered Terminal, or sealed FailStop"]
#[cfg(target_arch = "riscv32")]
pub enum PassiveScanControllerCancellationStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Waiting(PassiveScanControllerCancellationPending<'runtime, S, SCHEDULER_CAPACITY>),
    Recovered(PassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Terminal result of one source-ordered DTM preparation transaction.
///
/// Every variant owns the exact role-specific success or lossless failure.
/// Admission and sequence samples never cross this boundary.
#[must_use = "the prepared item or exact retry owner must be handled"]
#[cfg(target_arch = "riscv32")]
pub enum DtmControllerPreparationOutcome {
    /// Initial transmitter preparation reached a terminal result.
    TransmitterFirst(
        Result<
            crate::scheduler::DtmEmptySchedulerMergePrepared<
                crate::le::dtm::DtmTransmitterEvent,
                crate::scheduler::DtmInitialSchedulerItemPhase,
            >,
            crate::scheduler::DtmControllerTxPreparationFailure,
        >,
    ),
    /// Initial receiver preparation reached a terminal result.
    ReceiverFirst(
        Result<
            crate::scheduler::DtmEmptySchedulerMergePrepared<
                crate::le::dtm::DtmReceiverEvent,
                crate::scheduler::DtmInitialSchedulerItemPhase,
            >,
            crate::scheduler::DtmControllerRxPreparationFailure,
        >,
    ),
    /// Recurring transmitter preparation reached a terminal result.
    TransmitterRecurring(
        Result<
            crate::scheduler::DtmEmptySchedulerMergePrepared<
                crate::le::dtm::DtmTransmitterEvent,
                crate::scheduler::DtmRecurringSchedulerItemPhase,
            >,
            crate::scheduler::DtmControllerTxRecurringPreparationFailure,
        >,
    ),
    /// Recurring receiver preparation reached a terminal result.
    ReceiverRecurring(
        Result<
            crate::scheduler::DtmEmptySchedulerMergePrepared<
                crate::le::dtm::DtmReceiverEvent,
                crate::scheduler::DtmRecurringSchedulerItemPhase,
            >,
            crate::scheduler::DtmControllerRxRecurringPreparationFailure,
        >,
    ),
}

#[cfg(target_arch = "riscv32")]
enum DtmControllerPreparationPhase {
    TransmitterFirstAlwaysAwakeTiming {
        owner: crate::le::dtm::DtmPreparedTxGraph,
        link_state: crate::DtmLinkStateReset,
        channel: crate::le::dtm::DtmChannel,
        phy: crate::le::dtm::DtmPhy,
        requested_interval_micros: u16,
        now: crate::controller::time::ControllerSchedulerNow,
    },
    ReceiverFirstAlwaysAwakeTiming {
        owner: crate::le::dtm::DtmReceiverCpuOwned,
        link_state: crate::DtmLinkStateReset,
        channel: crate::le::dtm::DtmChannel,
        phy: crate::le::dtm::DtmPhy,
        now: crate::controller::time::ControllerSchedulerNow,
    },
    ReceiverRecurringAlwaysAwakeTiming {
        owner: crate::le::dtm::DtmActiveReceiverCpuOwned,
        epoch: crate::ControllerSchedulerEpoch,
    },
    ReceiverRecurringCurrent {
        owner: crate::le::dtm::DtmActiveReceiverCpuOwned,
        epoch: crate::ControllerSchedulerEpoch,
        timing_ready: crate::AlwaysAwakeTimingReady,
    },
    TransmitterFirstAdmission(crate::scheduler::core::DtmTransmitterFirstStaged),
    ReceiverFirstAdmission(crate::scheduler::core::DtmReceiverFirstStaged),
    TransmitterFirstSequence(crate::scheduler::core::DtmTransmitterFirstPreSequence),
    ReceiverFirstSequence(crate::scheduler::core::DtmReceiverFirstPreSequence),
    TransmitterRecurringSequence(crate::scheduler::core::DtmTransmitterRecurringPreSequence),
    ReceiverRecurringSequence(crate::scheduler::core::DtmReceiverRecurringPreSequence),
}

#[cfg(target_arch = "riscv32")]
struct DtmControllerPreparationTimeOwner<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<DtmControllerPreparationPhase>,
    cancelled: Option<DtmControllerPreparationOutcome>,
}

/// One exact post-enable timing, current, admission or sequence-time request.
///
/// Initial operations acquire post-enable timing after their current, then
/// admission before reservation and sequence only after reservation. Recurring
/// RX acquires post-enable timing before current; recurring TX starts after
/// current without that phase. Explicit cancellation returns the task owner and
/// releases any retained reservation. Dropping cancels the exact latch request
/// but also drops the sole task owner as a deliberate fail-stop; the long-lived
/// runner must therefore retain this state and use its explicit cancellation
/// edge.
#[must_use = "recheck, cancel, or drop the exact DTM time request"]
#[cfg(target_arch = "riscv32")]
pub struct DtmControllerPreparationPending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    core: ControllerTimePendingCore<
        DtmControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Terminal DTM preparation result with the exact Controller epoch retained.
#[must_use = "the Controller and role-specific preparation result must be handled"]
#[cfg(target_arch = "riscv32")]
pub struct DtmControllerPreparationTerminal<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: DtmControllerPreparationOutcome,
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
pub enum DtmControllerInitialPreparationFailure<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Another DTM session already owns the composition graph.
    SessionActive(ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>),
    /// The checked-out graph reached a lower preparation terminal.
    PreparationTerminal(DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of one bounded DTM controller-time phase observation.
#[must_use = "retain Pending or consume the terminal Controller and DTM result"]
#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc affine variants retain the complete in-flight or terminal DTM transaction"
)]
pub enum DtmControllerPreparationStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Hardware still owns the exact phase request.
    Pending(DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>),
    /// Preparation completed or failed with every affine owner returned.
    Terminal(DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of rechecking one exact post-enable controller-time request.
#[must_use = "retain Waiting or consume the Controller-bound Ready proof"]
#[cfg(target_arch = "riscv32")]
pub enum AlwaysAwakePostEnableTimeStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Hardware still owns the same request and complete Controller owner.
    Waiting(AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact request completed with a Controller-bound private sample.
    Ready(AlwaysAwakeTimeObservedAfterEnable<'runtime, S, SCHEDULER_CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform exactly one observation of this exact latch request.
    pub fn recheck(
        self,
    ) -> Result<
        AlwaysAwakePostEnableTimeStep<'runtime, S, SCHEDULER_CAPACITY>,
        AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.core.recheck() {
            Ok(ControllerTimePendingCoreStep::Waiting(core)) => {
                Ok(AlwaysAwakePostEnableTimeStep::Waiting(Self { core }))
            }
            Ok(ControllerTimePendingCoreStep::Ready { owner, sample }) => Ok(
                AlwaysAwakePostEnableTimeStep::Ready(AlwaysAwakeTimeObservedAfterEnable {
                    controller: owner,
                    sample,
                }),
            ),
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(AlwaysAwakePostEnableTimeFailure {
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
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.core.cancel() {
            Ok(controller) => Ok(controller),
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(AlwaysAwakePostEnableTimeFailure {
                    controller,
                    error: error.into(),
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: LegacyAdvertisingControllerPreparationOutcome,
    ) -> LegacyAdvertisingControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyAdvertisingControllerPreparationStep::Terminal(
            LegacyAdvertisingControllerPreparationTerminal {
                controller: ControllerSchedulerEpochRetained { controller },
                outcome,
            },
        )
    }

    fn fail_stop(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: LegacyAdvertisingControllerPreparationFailStopCause,
        cancelled: Option<crate::le::advertising::LegacyAdvertisingCancelled<'static>>,
    ) -> LegacyAdvertisingControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyAdvertisingControllerPreparationStep::FailStop(
            LegacyAdvertisingControllerPreparationFailStop::active(controller, cause, cancelled),
        )
    }

    fn rollback_after_idle(
        mut controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
        error: LegacyAdvertisingControllerPreparationError,
    ) -> LegacyAdvertisingControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        match controller.restore_legacy_advertising_cancelled(cancelled) {
            timed_preparation::TimedPreparationRollbackOutcome::Restored => Self::terminal(
                controller,
                LegacyAdvertisingControllerPreparationOutcome::Rejected(error),
            ),
            timed_preparation::TimedPreparationRollbackOutcome::FailStop(cancelled) => {
                Self::fail_stop(
                    controller,
                    LegacyAdvertisingControllerPreparationFailStopCause::RuntimeRestore,
                    Some(cancelled),
                )
            }
        }
    }

    /// Perform one bounded observation of the exact advertising time request.
    pub fn recheck(
        self,
    ) -> LegacyAdvertisingControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut controller, phase, sample) = match self.timed.recheck() {
            timed_preparation::TimedPreparationStep::Waiting(timed) => {
                return LegacyAdvertisingControllerPreparationStep::Pending(Self { timed });
            }
            timed_preparation::TimedPreparationStep::Ready {
                controller,
                phase,
                sample,
            } => (controller, phase, sample),
            timed_preparation::TimedPreparationStep::FailStop(failure) => {
                return LegacyAdvertisingControllerPreparationStep::FailStop(
                    LegacyAdvertisingControllerPreparationFailStop::from_timed(failure),
                );
            }
        };
        match phase {
            LegacyAdvertisingControllerPreparationPhase::AlwaysAwakeTiming { reset, now } => {
                let epoch = now.epoch();
                let current = crate::SchedulerInstant::from_image(now.micros());
                let radio_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample)
                    .into_scheduler_instant();
                let timing = crate::le::advertising::LegacyAdvertisingTimingObservation {
                    current,
                    radio_ready,
                    epoch,
                };
                let candidate = match reset
                    .form_first_event_candidate(timing, controller.runtime.scheduler_config())
                {
                    crate::le::advertising::LegacyAdvertisingFirstEventCandidateOutcome::Candidate(
                        candidate,
                    ) => candidate,
                    crate::le::advertising::LegacyAdvertisingFirstEventCandidateOutcome::TimingRejected(
                        reset,
                    ) => {
                        return Self::rollback_after_idle(
                            controller,
                            reset.cancel(),
                            LegacyAdvertisingControllerPreparationError::TimingWindow,
                        );
                    }
                };
                match controller.begin_legacy_advertising_preparation_time(
                    LegacyAdvertisingControllerPreparationPhase::Admission(candidate),
                ) {
                    Ok(pending) => LegacyAdvertisingControllerPreparationStep::Pending(pending),
                    Err(fail_stop) => {
                        LegacyAdvertisingControllerPreparationStep::FailStop(fail_stop)
                    }
                }
            }
            LegacyAdvertisingControllerPreparationPhase::Admission(candidate) => {
                let admitted = match controller.runtime.admit_legacy_advertising_first_event(
                    candidate,
                    crate::scheduler::LegacyAdvertisingAdmissionObservation { sample },
                ) {
                    Ok(admitted) => admitted,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle(
                            controller,
                            failure.into_candidate().cancel(),
                            LegacyAdvertisingControllerPreparationError::Event(error),
                        );
                    }
                };
                match controller.begin_legacy_advertising_preparation_time(
                    LegacyAdvertisingControllerPreparationPhase::Sequence(admitted),
                ) {
                    Ok(pending) => LegacyAdvertisingControllerPreparationStep::Pending(pending),
                    Err(fail_stop) => {
                        LegacyAdvertisingControllerPreparationStep::FailStop(fail_stop)
                    }
                }
            }
            LegacyAdvertisingControllerPreparationPhase::Sequence(admitted) => {
                let prepared = match controller.runtime.prepare_legacy_advertising_first_event(
                    admitted,
                    crate::scheduler::LegacyAdvertisingSequenceObservation { sample },
                ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle(
                            controller,
                            failure.into_candidate().cancel(),
                            LegacyAdvertisingControllerPreparationError::Event(error),
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_legacy_advertising_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::terminal(
                        controller,
                        LegacyAdvertisingControllerPreparationOutcome::Prepared(merged),
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let cancelled = controller
                            .runtime
                            .cancel_legacy_advertising_first_event(failure.into_prepared());
                        Self::rollback_after_idle(
                            controller,
                            cancelled,
                            LegacyAdvertisingControllerPreparationError::EmptyList(error),
                        )
                    }
                }
            }
        }
    }

    /// Cancel the exact unpublished phase and restore the advertising runtime.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn cancel(
        self,
    ) -> Result<
        LegacyAdvertisingControllerCancellationPending<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        self.timed
            .cancel()
            .map(|timed| LegacyAdvertisingControllerCancellationPending { timed })
            .map_err(LegacyAdvertisingControllerPreparationFailStop::from_timed)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> timed_preparation::TimedPreparationController
    for ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<ControllerTimeRequest, crate::scheduler::ControllerTimeAcquisitionError> {
        self.runtime
            .request_controller_time()
            .map_err(controller_time_begin_error)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingControllerCancellationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    pub fn recheck(
        self,
    ) -> LegacyAdvertisingControllerCancellationStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self
            .timed
            .recheck::<crate::le::advertising::LegacyAdvertisingCancelled<'static>>()
        {
            timed_preparation::TimedPreparationCancellationStep::Waiting(timed) => {
                LegacyAdvertisingControllerCancellationStep::Waiting(Self { timed })
            }
            timed_preparation::TimedPreparationCancellationStep::Recovered(controller) => {
                LegacyAdvertisingControllerCancellationStep::Recovered(
                    LegacyAdvertisingControllerPreparationTerminal {
                        controller: ControllerSchedulerEpochRetained { controller },
                        outcome: LegacyAdvertisingControllerPreparationOutcome::Rejected(
                            LegacyAdvertisingControllerPreparationError::Cancelled,
                        ),
                    },
                )
            }
            timed_preparation::TimedPreparationCancellationStep::FailStop(failure) => {
                LegacyAdvertisingControllerCancellationStep::FailStop(
                    LegacyAdvertisingControllerPreparationFailStop::from_timed(failure),
                )
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: PassiveScanControllerPreparationOutcome,
    ) -> PassiveScanControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        PassiveScanControllerPreparationStep::Terminal(PassiveScanControllerPreparationTerminal {
            controller: ControllerSchedulerEpochRetained { controller },
            outcome,
        })
    }

    fn fail_stop(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: PassiveScanControllerPreparationFailStopCause,
        graph: Option<oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned>,
    ) -> PassiveScanControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        PassiveScanControllerPreparationStep::FailStop(
            PassiveScanControllerPreparationFailStop::active(controller, cause, graph),
        )
    }

    fn rollback_after_idle(
        mut controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        graph: oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
        error: PassiveScanControllerPreparationError,
    ) -> PassiveScanControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        match controller.restore_passive_scan_graph(graph) {
            timed_preparation::TimedPreparationRollbackOutcome::Restored => Self::terminal(
                controller,
                PassiveScanControllerPreparationOutcome::Rejected(error),
            ),
            timed_preparation::TimedPreparationRollbackOutcome::FailStop(graph) => Self::fail_stop(
                controller,
                PassiveScanControllerPreparationFailStopCause::RuntimeRestore,
                Some(graph),
            ),
        }
    }

    /// Perform one bounded observation of the exact scanner time request.
    pub fn recheck(self) -> PassiveScanControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut controller, phase, sample) = match self.timed.recheck() {
            timed_preparation::TimedPreparationStep::Waiting(timed) => {
                return PassiveScanControllerPreparationStep::Pending(Self { timed });
            }
            timed_preparation::TimedPreparationStep::Ready {
                controller,
                phase,
                sample,
            } => (controller, phase, sample),
            timed_preparation::TimedPreparationStep::FailStop(failure) => {
                return PassiveScanControllerPreparationStep::FailStop(
                    PassiveScanControllerPreparationFailStop::from_timed(failure),
                );
            }
        };
        match phase {
            PassiveScanControllerPreparationPhase::AlwaysAwakeTiming {
                graph,
                channel,
                parameters,
                previous_phase,
                now,
            } => {
                let epoch = now.epoch();
                let controller_time = sample.latched_time();
                let radio_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample)
                    .into_scheduler_instant();
                let timing = crate::le::scanning::passive::timing::PassiveScanTimingObservation {
                    current: crate::SchedulerInstant::from_image(now.micros()),
                    radio_ready,
                    epoch,
                    controller_time,
                };
                let candidate = match previous_phase {
                    Some(previous) => timing.form_recurring_event_candidate(
                        graph,
                        channel,
                        parameters,
                        controller.runtime.scheduler_config(),
                        previous,
                    ),
                    None => timing.form_first_event_candidate(
                        graph,
                        channel,
                        parameters,
                        controller.runtime.scheduler_config(),
                    ),
                };
                let (candidate, phase) = match candidate {
                    Ok(candidate) => candidate,
                    Err(failure) => {
                        return Self::rollback_after_idle(
                            controller,
                            failure.into_graph(),
                            PassiveScanControllerPreparationError::TimingWindow,
                        );
                    }
                };
                match controller.begin_passive_scan_preparation_time(
                    PassiveScanControllerPreparationPhase::Admission { candidate, phase },
                ) {
                    Ok(pending) => PassiveScanControllerPreparationStep::Pending(pending),
                    Err(fail_stop) => PassiveScanControllerPreparationStep::FailStop(fail_stop),
                }
            }
            PassiveScanControllerPreparationPhase::Admission { candidate, phase } => {
                let admitted = match controller.runtime.admit_passive_scan_first_event(
                    candidate,
                    crate::scheduler::PassiveScanAdmissionObservation { sample },
                ) {
                    Ok(admitted) => admitted,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle(
                            controller,
                            failure.into_candidate().cancel(),
                            PassiveScanControllerPreparationError::Event(error),
                        );
                    }
                };
                match controller.begin_passive_scan_preparation_time(
                    PassiveScanControllerPreparationPhase::Sequence { admitted, phase },
                ) {
                    Ok(pending) => PassiveScanControllerPreparationStep::Pending(pending),
                    Err(fail_stop) => PassiveScanControllerPreparationStep::FailStop(fail_stop),
                }
            }
            PassiveScanControllerPreparationPhase::Sequence { admitted, phase } => {
                let prepared = match controller.runtime.prepare_passive_scan_first_event(
                    admitted,
                    crate::scheduler::PassiveScanSequenceObservation { sample },
                ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle(
                            controller,
                            failure.into_candidate().cancel(),
                            PassiveScanControllerPreparationError::Event(error),
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_passive_scan_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::terminal(
                        controller,
                        PassiveScanControllerPreparationOutcome::Prepared { merged, phase },
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let graph = controller
                            .runtime
                            .cancel_passive_scan_first_event(failure.into_prepared());
                        Self::rollback_after_idle(
                            controller,
                            graph,
                            PassiveScanControllerPreparationError::EmptyList(error),
                        )
                    }
                }
            }
        }
    }

    /// Cancel the unpublished phase and return the graph to its sole runtime.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn cancel(
        self,
    ) -> Result<
        PassiveScanControllerCancellationPending<'runtime, S, SCHEDULER_CAPACITY>,
        PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        self.timed
            .cancel()
            .map(|timed| PassiveScanControllerCancellationPending { timed })
            .map_err(PassiveScanControllerPreparationFailStop::from_timed)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanControllerCancellationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    pub fn recheck(self) -> PassiveScanControllerCancellationStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self
            .timed
            .recheck::<oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned>()
        {
            timed_preparation::TimedPreparationCancellationStep::Waiting(timed) => {
                PassiveScanControllerCancellationStep::Waiting(Self { timed })
            }
            timed_preparation::TimedPreparationCancellationStep::Recovered(controller) => {
                PassiveScanControllerCancellationStep::Recovered(
                    PassiveScanControllerPreparationTerminal {
                        controller: ControllerSchedulerEpochRetained { controller },
                        outcome: PassiveScanControllerPreparationOutcome::Rejected(
                            PassiveScanControllerPreparationError::Cancelled,
                        ),
                    },
                )
            }
            timed_preparation::TimedPreparationCancellationStep::FailStop(failure) => {
                PassiveScanControllerCancellationStep::FailStop(
                    PassiveScanControllerPreparationFailStop::from_timed(failure),
                )
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Borrow the exact role-specific terminal result.
    pub const fn outcome(&self) -> &DtmControllerPreparationOutcome {
        &self.outcome
    }

    /// Recover the retained Controller epoch and role-specific terminal result.
    pub fn into_parts(
        self,
    ) -> (
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: DtmControllerPreparationOutcome,
    ) -> DtmControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        DtmControllerPreparationStep::Terminal(DtmControllerPreparationTerminal {
            controller: ControllerSchedulerEpochRetained { controller },
            outcome,
        })
    }

    /// Perform one bounded observation of the current DTM time request.
    ///
    /// Completing an initial admission reserves the resolved window and only
    /// then publishes the sequence request. Consequently one call may advance
    /// into another `Pending` state without yet producing a terminal result.
    pub fn recheck(self) -> DtmControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(ControllerTimePendingCoreStep::Waiting(core)) => {
                return DtmControllerPreparationStep::Pending(Self { core });
            }
            Ok(ControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let phase = owner
                    .phase
                    .take()
                    .expect("failed DTM time recheck retains its exact phase");
                let outcome = owner
                    .controller
                    .cancel_dtm_preparation_phase(phase, controller_time_event_error(error));
                return Self::terminal(owner.controller, outcome);
            }
        };
        let phase = owner
            .phase
            .take()
            .expect("completed DTM time request retains its exact phase");
        let mut controller = owner.controller;
        match phase {
            DtmControllerPreparationPhase::TransmitterFirstAlwaysAwakeTiming {
                owner,
                link_state,
                channel,
                phy,
                requested_interval_micros,
                now,
            } => {
                let timing_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(now.epoch(), sample);
                let staged = match controller.runtime.stage_dtm_transmitter_first_item(
                    owner,
                    link_state,
                    channel,
                    phy,
                    requested_interval_micros,
                    now,
                    timing_ready,
                ) {
                    Ok(staged) => staged,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            DtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
                        );
                    }
                };
                match controller.begin_dtm_preparation_time(
                    DtmControllerPreparationPhase::TransmitterFirstAdmission(staged),
                ) {
                    Ok(pending) => DtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => DtmControllerPreparationStep::Terminal(terminal),
                }
            }
            DtmControllerPreparationPhase::ReceiverFirstAlwaysAwakeTiming {
                owner,
                link_state,
                channel,
                phy,
                now,
            } => {
                let timing_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(now.epoch(), sample);
                let staged = match controller.runtime.stage_dtm_receiver_first_item(
                    owner,
                    link_state,
                    channel,
                    phy,
                    now,
                    timing_ready,
                ) {
                    Ok(staged) => staged,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            DtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
                        );
                    }
                };
                match controller.begin_dtm_preparation_time(
                    DtmControllerPreparationPhase::ReceiverFirstAdmission(staged),
                ) {
                    Ok(pending) => DtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => DtmControllerPreparationStep::Terminal(terminal),
                }
            }
            DtmControllerPreparationPhase::ReceiverRecurringAlwaysAwakeTiming { owner, epoch } => {
                let timing_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample);
                match controller.begin_dtm_preparation_time(
                    DtmControllerPreparationPhase::ReceiverRecurringCurrent {
                        owner,
                        epoch,
                        timing_ready,
                    },
                ) {
                    Ok(pending) => DtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => DtmControllerPreparationStep::Terminal(terminal),
                }
            }
            DtmControllerPreparationPhase::ReceiverRecurringCurrent {
                owner,
                epoch,
                timing_ready,
            } => {
                let epoch = epoch.reanchor(&sample);
                *controller.scheduler_epoch = Some(epoch);
                let now = crate::controller::time::ControllerSchedulerNow::from_retained_epoch(
                    epoch, sample,
                );
                let staged = match controller.runtime.stage_dtm_receiver_recurring_item(
                    owner,
                    now,
                    timing_ready,
                ) {
                    Ok(staged) => staged,
                    Err(failure) => {
                        return Self::terminal(
                            controller,
                            DtmControllerPreparationOutcome::ReceiverRecurring(Err(failure)),
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
                            DtmControllerPreparationOutcome::ReceiverRecurring(Err(failure)),
                        );
                    }
                };
                match controller.begin_dtm_preparation_time(
                    DtmControllerPreparationPhase::ReceiverRecurringSequence(pre_sequence),
                ) {
                    Ok(pending) => DtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => DtmControllerPreparationStep::Terminal(terminal),
                }
            }
            DtmControllerPreparationPhase::TransmitterFirstAdmission(staged) => {
                match controller
                    .runtime
                    .admit_dtm_transmitter_first_item(staged, sample)
                {
                    Ok(pre_sequence) => match controller.begin_dtm_preparation_time(
                        DtmControllerPreparationPhase::TransmitterFirstSequence(pre_sequence),
                    ) {
                        Ok(pending) => DtmControllerPreparationStep::Pending(pending),
                        Err(terminal) => DtmControllerPreparationStep::Terminal(terminal),
                    },
                    Err(failure) => Self::terminal(
                        controller,
                        DtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
                    ),
                }
            }
            DtmControllerPreparationPhase::ReceiverFirstAdmission(staged) => {
                match controller
                    .runtime
                    .admit_dtm_receiver_first_item(staged, sample)
                {
                    Ok(pre_sequence) => match controller.begin_dtm_preparation_time(
                        DtmControllerPreparationPhase::ReceiverFirstSequence(pre_sequence),
                    ) {
                        Ok(pending) => DtmControllerPreparationStep::Pending(pending),
                        Err(terminal) => DtmControllerPreparationStep::Terminal(terminal),
                    },
                    Err(failure) => Self::terminal(
                        controller,
                        DtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
                    ),
                }
            }
            DtmControllerPreparationPhase::TransmitterFirstSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_transmitter_first_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    DtmControllerPreparationOutcome::TransmitterFirst(result),
                )
            }
            DtmControllerPreparationPhase::ReceiverFirstSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_receiver_first_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    DtmControllerPreparationOutcome::ReceiverFirst(result),
                )
            }
            DtmControllerPreparationPhase::TransmitterRecurringSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_transmitter_recurring_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    DtmControllerPreparationOutcome::TransmitterRecurring(result),
                )
            }
            DtmControllerPreparationPhase::ReceiverRecurringSequence(pre_sequence) => {
                let result = controller
                    .runtime
                    .finish_dtm_receiver_recurring_item(pre_sequence, sample);
                Self::terminal(
                    controller,
                    DtmControllerPreparationOutcome::ReceiverRecurring(result),
                )
            }
        }
    }

    /// Cancel the exact phase, release any reservation and recover retry ownership.
    pub fn cancel(self) -> DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY> {
        let mut owner = match self.core.cancel() {
            Ok(owner) => owner,
            Err(failure) => failure.into_parts().0,
        };
        let outcome = owner
            .cancelled
            .take()
            .expect("explicit DTM time cancellation records its lossless outcome");
        DtmControllerPreparationTerminal {
            controller: ControllerSchedulerEpochRetained {
                controller: owner.controller,
            },
            outcome,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> ControllerTimePendingOwner
    for DtmControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
{
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        ControllerTimePendingOwner::recheck_owned_controller_time(&mut self.controller, request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        let result =
            ControllerTimePendingOwner::cancel_owned_controller_time(&mut self.controller, request);
        let error = match result {
            Ok(()) => crate::scheduler::ControllerTimeAcquisitionError::Cancelled,
            Err(error) => controller_time_event_error(error),
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
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        ControllerTimePendingOwner::drain_orphan_controller_time(&mut self.controller)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>
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
        ControllerSchedulerCurrentStep<'runtime, S, SCHEDULER_CAPACITY>,
        ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let epoch = self.epoch;
        match self.core.recheck() {
            Ok(ControllerTimePendingCoreStep::Waiting(core)) => {
                Ok(ControllerSchedulerCurrentStep::Waiting(Self {
                    core,
                    epoch,
                }))
            }
            Ok(ControllerTimePendingCoreStep::Ready { owner, sample }) => {
                let epoch = epoch.reanchor(&sample);
                *owner.scheduler_epoch = Some(epoch);
                Ok(ControllerSchedulerCurrentStep::Ready(
                    ControllerSchedulerNowReady {
                        controller: owner,
                        epoch,
                        sample,
                    },
                ))
            }
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(ControllerSchedulerCurrentFailure {
                    controller: ControllerSchedulerEpochRetained { controller },
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
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.core.cancel() {
            Ok(controller) => Ok(ControllerSchedulerEpochRetained { controller }),
            Err(failure) => {
                let (controller, error) = failure.into_parts();
                Err(ControllerSchedulerCurrentFailure {
                    controller: ControllerSchedulerEpochRetained { controller },
                    error: error.into(),
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    fn restore_legacy_advertising_cancelled(
        &mut self,
        cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    ) -> timed_preparation::TimedPreparationRollbackOutcome<
        crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    > {
        match self
            .legacy_advertising_resources
            .restore_cancelled(cancelled)
        {
            crate::LegacyAdvertisingCancelledRestoreOutcome::Restored => {
                timed_preparation::TimedPreparationRollbackOutcome::Restored
            }
            crate::LegacyAdvertisingCancelledRestoreOutcome::Rejected(cancelled) => {
                timed_preparation::TimedPreparationRollbackOutcome::FailStop(cancelled)
            }
        }
    }

    fn cancel_legacy_advertising_preparation_phase(
        &mut self,
        phase: LegacyAdvertisingControllerPreparationPhase,
    ) -> timed_preparation::TimedPreparationRollbackOutcome<
        crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    > {
        let cancelled = match phase {
            LegacyAdvertisingControllerPreparationPhase::AlwaysAwakeTiming { reset, .. } => {
                reset.cancel()
            }
            LegacyAdvertisingControllerPreparationPhase::Admission(candidate) => candidate.cancel(),
            LegacyAdvertisingControllerPreparationPhase::Sequence(admitted) => self
                .runtime
                .cancel_legacy_advertising_first_pre_sequence(admitted),
        };
        self.restore_legacy_advertising_cancelled(cancelled)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc begin rejection retains the restored task and typed outcome"
    )]
    fn begin_legacy_advertising_preparation_time(
        self,
        phase: LegacyAdvertisingControllerPreparationPhase,
    ) -> Result<
        LegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        timed_preparation::TimedPreparationPending::begin(
            self,
            phase,
            ControllerPublishedTaskService::cancel_legacy_advertising_preparation_phase,
        )
        .map(|timed| LegacyAdvertisingControllerPreparationPending { timed })
        .map_err(LegacyAdvertisingControllerPreparationFailStop::from_timed)
    }

    fn restore_passive_scan_graph(
        &mut self,
        graph: oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
    ) -> timed_preparation::TimedPreparationRollbackOutcome<
        oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
    > {
        match self.passive_scan_resources.restore_idle(graph) {
            Ok(()) => timed_preparation::TimedPreparationRollbackOutcome::Restored,
            Err(graph) => timed_preparation::TimedPreparationRollbackOutcome::FailStop(graph),
        }
    }

    fn cancel_passive_scan_preparation_phase(
        &mut self,
        phase: PassiveScanControllerPreparationPhase,
    ) -> timed_preparation::TimedPreparationRollbackOutcome<
        oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
    > {
        let graph = match phase {
            PassiveScanControllerPreparationPhase::AlwaysAwakeTiming { graph, .. } => graph,
            PassiveScanControllerPreparationPhase::Admission { candidate, .. } => {
                candidate.cancel()
            }
            PassiveScanControllerPreparationPhase::Sequence { admitted, .. } => self
                .runtime
                .cancel_passive_scan_first_pre_sequence(admitted),
        };
        self.restore_passive_scan_graph(graph)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the Controller and restored scanner graph"
    )]
    fn begin_passive_scan_preparation_time(
        self,
        phase: PassiveScanControllerPreparationPhase,
    ) -> Result<
        PassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        timed_preparation::TimedPreparationPending::begin(
            self,
            phase,
            ControllerPublishedTaskService::cancel_passive_scan_preparation_phase,
        )
        .map(|timed| PassiveScanControllerPreparationPending { timed })
        .map_err(PassiveScanControllerPreparationFailStop::from_timed)
    }

    /// Return the exact cancelled or stopped session graph to this Controller.
    ///
    /// The embedded runtime rejects an occupied slot or a graph minted from
    /// another pinned storage object and returns that owner unchanged. This is
    /// the only public graph-return edge after final Controller publication;
    /// initial checkout remains private to the typed TX/RX start operations.
    pub fn restore_dtm_session_idle(
        &mut self,
        idle: crate::le::dtm::DtmSessionIdle,
    ) -> Result<(), crate::le::dtm::DtmSessionIdle> {
        self.dtm_resources.restore_idle(idle)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection returns the complete affine advertising event"
    )]
    pub(crate) fn restore_legacy_advertising_completed_disabled(
        &mut self,
        completed: crate::le::advertising::LegacyAdvertisingEventCompleted<'static>,
    ) -> Result<(), crate::le::advertising::LegacyAdvertisingEventCompleted<'static>> {
        self.legacy_advertising_resources
            .restore_completed_disabled(completed)
    }

    fn new_dtm_link_state_reset(&self, role: crate::le::dtm::DtmRole) -> crate::DtmLinkStateReset {
        crate::DtmLinkStateReset::new(self.dtm_resources.default_tx_power_dbm(), role)
    }

    fn cancel_dtm_preparation_phase(
        &mut self,
        phase: DtmControllerPreparationPhase,
        error: crate::scheduler::ControllerTimeAcquisitionError,
    ) -> DtmControllerPreparationOutcome {
        match phase {
            DtmControllerPreparationPhase::TransmitterFirstAlwaysAwakeTiming { owner, .. } => {
                DtmControllerPreparationOutcome::TransmitterFirst(Err(self
                    .runtime
                    .reject_dtm_transmitter_first_before_stage(owner, error)))
            }
            DtmControllerPreparationPhase::ReceiverFirstAlwaysAwakeTiming { owner, .. } => {
                DtmControllerPreparationOutcome::ReceiverFirst(Err(self
                    .runtime
                    .reject_dtm_receiver_first_before_stage(owner, error)))
            }
            DtmControllerPreparationPhase::ReceiverRecurringAlwaysAwakeTiming { owner, .. }
            | DtmControllerPreparationPhase::ReceiverRecurringCurrent { owner, .. } => {
                DtmControllerPreparationOutcome::ReceiverRecurring(Err(self
                    .runtime
                    .reject_dtm_receiver_recurring_before_stage(owner, error)))
            }
            DtmControllerPreparationPhase::TransmitterFirstAdmission(staged) => {
                DtmControllerPreparationOutcome::TransmitterFirst(Err(self
                    .runtime
                    .cancel_dtm_transmitter_first_staged(staged, error)))
            }
            DtmControllerPreparationPhase::ReceiverFirstAdmission(staged) => {
                DtmControllerPreparationOutcome::ReceiverFirst(Err(self
                    .runtime
                    .cancel_dtm_receiver_first_staged(staged, error)))
            }
            DtmControllerPreparationPhase::TransmitterFirstSequence(pre_sequence) => {
                DtmControllerPreparationOutcome::TransmitterFirst(Err(self
                    .runtime
                    .cancel_dtm_transmitter_first_pre_sequence(pre_sequence, error)))
            }
            DtmControllerPreparationPhase::ReceiverFirstSequence(pre_sequence) => {
                DtmControllerPreparationOutcome::ReceiverFirst(Err(self
                    .runtime
                    .cancel_dtm_receiver_first_pre_sequence(pre_sequence, error)))
            }
            DtmControllerPreparationPhase::TransmitterRecurringSequence(pre_sequence) => {
                DtmControllerPreparationOutcome::TransmitterRecurring(Err(self
                    .runtime
                    .cancel_dtm_transmitter_recurring_pre_sequence(pre_sequence, error)))
            }
            DtmControllerPreparationPhase::ReceiverRecurringSequence(pre_sequence) => {
                DtmControllerPreparationOutcome::ReceiverRecurring(Err(self
                    .runtime
                    .cancel_dtm_receiver_recurring_pre_sequence(pre_sequence, error)))
            }
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete role retry owner"
    )]
    fn begin_dtm_always_awake_timing(
        mut self,
        phase: DtmControllerPreparationPhase,
    ) -> Result<
        DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome =
                    self.cancel_dtm_preparation_phase(phase, controller_time_begin_error(error));
                return Err(DtmControllerPreparationTerminal {
                    controller: ControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(DtmControllerPreparationPending {
            core: ControllerTimePendingCore::new(
                DtmControllerPreparationTimeOwner {
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
        phase: DtmControllerPreparationPhase,
    ) -> Result<
        DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome =
                    self.cancel_dtm_preparation_phase(phase, controller_time_begin_error(error));
                return Err(DtmControllerPreparationTerminal {
                    controller: ControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(DtmControllerPreparationPending {
            core: ControllerTimePendingCore::new(
                DtmControllerPreparationTimeOwner {
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
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> ControllerTimePendingOwner
    for ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        match self.runtime.recheck_owned_controller_time(request) {
            Ok(ControllerTimeEventStep::Waiting) => Ok(ControllerTimePendingOwnerStep::Waiting),
            Ok(ControllerTimeEventStep::Sample {
                request: completed,
                sample,
            }) if completed == request => Ok(ControllerTimePendingOwnerStep::Ready(sample)),
            Ok(
                ControllerTimeEventStep::Idle
                | ControllerTimeEventStep::OrphanDrained
                | ControllerTimeEventStep::Sample { .. },
            ) => Err(ControllerTimeEventError::RequestMismatch),
            Err(error) => Err(error),
        }
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        self.runtime.cancel_owned_controller_time(request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        match self.runtime.drain_orphan_controller_time() {
            Ok(ControllerTimeEventStep::Idle) => Ok(ControllerTimePendingOrphanStep::Idle),
            Ok(ControllerTimeEventStep::Waiting) => Ok(ControllerTimePendingOrphanStep::Waiting),
            Ok(ControllerTimeEventStep::OrphanDrained) => {
                Ok(ControllerTimePendingOrphanStep::Drained)
            }
            Ok(ControllerTimeEventStep::Sample { .. }) => {
                Err(ControllerTimeEventError::RequestMismatch)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    AlwaysAwakeTimeObservedAfterEnable<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Initialize this Controller's persistent scheduler epoch from the first
    /// live post-enable sample.
    ///
    /// The same affine sample is retained as the sole current-time authority
    /// for one DTM preparation attempt. This transition proves neither RF
    /// readiness nor deadline readiness.
    pub fn initialize_scheduler_epoch(
        self,
    ) -> ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY> {
        let epoch = crate::ControllerSchedulerEpoch::from_first_live_update(
            &self.sample,
            self.controller.runtime.controller_time_scale(),
        );
        *self.controller.scheduler_epoch = Some(epoch);
        ControllerSchedulerNowReady {
            controller: self.controller,
            epoch,
            sample: self.sample,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>
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
        ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
        ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let Some(epoch) = *self.controller.scheduler_epoch else {
            return Err(ControllerSchedulerCurrentBeginFailure {
                controller: self,
                error: ControllerSchedulerCurrentBeginError::EpochUnavailable,
            });
        };
        let request = match self.controller.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                return Err(ControllerSchedulerCurrentBeginFailure {
                    controller: self,
                    error: error.into(),
                });
            }
        };
        let controller = self.controller;
        Ok(ControllerSchedulerCurrentPending {
            core: ControllerTimePendingCore::new(controller, request),
            epoch,
        })
    }

    /// Begin recurring receiver preparation in vendor
    /// post-enable-before-current order.
    ///
    /// The retained always-awake BLE-PHY owner first publishes a private
    /// post-enable timing request. Only its completed microsecond-domain result
    /// can advance to a second fresh-current request and reanchor this epoch.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete active RX owner"
    )]
    pub fn begin_dtm_receiver_recurring_item(
        self,
        owner: crate::le::dtm::DtmActiveReceiverCpuOwned,
    ) -> Result<
        DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let controller = self.controller;
        let epoch = (*controller.scheduler_epoch)
            .expect("the retained scheduler epoch cannot lose its stored epoch");
        controller.begin_dtm_always_awake_timing(
            DtmControllerPreparationPhase::ReceiverRecurringAlwaysAwakeTiming { owner, epoch },
        )
    }

    /// Perform one bounded observation of an abandoned fresh-current request.
    ///
    /// `Waiting` requires one later call. A completed orphan is discarded and
    /// never advances the retained epoch or becomes a sample for a later DTM
    /// preparation. Every outcome preserves this exact retained owner.
    pub fn drain_abandoned_controller_time(
        &mut self,
    ) -> Result<ControllerTimeOrphanDrainStep, ControllerSchedulerCurrentError> {
        match drain_controller_time_orphan(&mut self.controller) {
            Ok(ControllerTimePendingOrphanStep::Idle) => Ok(ControllerTimeOrphanDrainStep::Idle),
            Ok(ControllerTimePendingOrphanStep::Waiting) => {
                Ok(ControllerTimeOrphanDrainStep::Waiting)
            }
            Ok(ControllerTimePendingOrphanStep::Drained) => {
                Ok(ControllerTimeOrphanDrainStep::Drained)
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
    ) -> ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY> {
        self.controller
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Discard this unused current sample while retaining the initialized
    /// scheduler epoch and the exact published task service.
    ///
    /// This is the lossless abort edge before a DTM preparation starts. The
    /// next attempt must acquire a fresh scheduler current; it cannot reuse
    /// the discarded sample.
    pub fn into_retained_epoch(
        self,
    ) -> ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY> {
        ControllerSchedulerEpochRetained {
            controller: self.controller,
        }
    }

    fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        crate::controller::time::ControllerSchedulerNow,
    ) {
        (
            self.controller,
            crate::controller::time::ControllerSchedulerNow::from_retained_epoch(
                self.epoch,
                self.sample,
            ),
        )
    }

    /// Apply one fresh sequence observation to an already reserved successor.
    pub(crate) fn finish_legacy_advertising_recurring_event(
        self,
        admitted: crate::scheduler::LegacyAdvertisingRecurringPreSequence<'static>,
    ) -> LegacyAdvertisingRecurringSequenceCompletion<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            mut controller,
            sample,
            ..
        } = self;
        let prepared = match controller
            .runtime
            .prepare_legacy_advertising_recurring_event(
                admitted,
                crate::scheduler::LegacyAdvertisingSequenceObservation { sample },
            ) {
            Ok(prepared) => prepared,
            Err(failure) => {
                return LegacyAdvertisingRecurringSequenceCompletion::EventRejected {
                    task: controller,
                    failure,
                };
            }
        };
        match controller
            .runtime
            .prepare_legacy_advertising_empty_list_merge(prepared)
        {
            Ok(merged) => LegacyAdvertisingRecurringSequenceCompletion::Prepared {
                task: controller,
                merged,
            },
            Err(failure) => LegacyAdvertisingRecurringSequenceCompletion::EmptyListRejected {
                task: controller,
                failure,
            },
        }
    }

    /// Begin the source-ordered first legacy-advertising transaction.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete current or preparation owner"
    )]
    pub(crate) fn begin_legacy_advertising_first_event(
        self,
        set: oer_bluetooth_ll::advertising::LegacyNonconnectableAdvertisingSet<'static>,
    ) -> Result<
        LegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyAdvertisingControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let mut current = self;
        let event = match current
            .controller
            .legacy_advertising_resources
            .begin_event(set)
        {
            Ok(event) => event,
            Err(error) => {
                return Err(
                    LegacyAdvertisingControllerInitialPreparationFailure::Rejected {
                        current,
                        error: LegacyAdvertisingControllerPreparationError::Runtime(error),
                    },
                );
            }
        };
        let (prepared, default_tx_power) = event.into_parts();
        let reset = match prepared.reset_link_state(default_tx_power) {
            crate::le::advertising::LegacyAdvertisingLinkStateResetOutcome::Reset(reset) => reset,
            crate::le::advertising::LegacyAdvertisingLinkStateResetOutcome::Rejected {
                prepared,
                error,
            } => {
                let cancelled = prepared.cancel();
                if let timed_preparation::TimedPreparationRollbackOutcome::FailStop(cancelled) =
                    current
                        .controller
                        .restore_legacy_advertising_cancelled(cancelled)
                {
                    return Err(
                        LegacyAdvertisingControllerInitialPreparationFailure::FailStop(
                            LegacyAdvertisingControllerPreparationFailStop {
                                cause: LegacyAdvertisingControllerPreparationFailStopCause::RuntimeRestore,
                                _state: LegacyAdvertisingControllerPreparationFailStopState::Initial {
                                    _current: current,
                                    _cancelled: cancelled,
                                },
                            },
                        ),
                    );
                }
                return Err(
                    LegacyAdvertisingControllerInitialPreparationFailure::Rejected {
                        current,
                        error: LegacyAdvertisingControllerPreparationError::LinkState(error),
                    },
                );
            }
        };
        let (controller, now) = current.into_parts();
        controller
            .begin_legacy_advertising_preparation_time(
                LegacyAdvertisingControllerPreparationPhase::AlwaysAwakeTiming { reset, now },
            )
            .map_err(LegacyAdvertisingControllerInitialPreparationFailure::FailStop)
    }

    /// Begin the source-ordered first passive scanner transaction.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete current or scanner owner"
    )]
    pub(crate) fn begin_passive_scan_first_event(
        self,
        parameters: oer_bluetooth_ll::scanning::LegacyPassiveScanParameters,
        channel: oer_bluetooth_ll::scanning::PrimaryScanChannel,
        previous_phase: Option<crate::le::scanning::PassiveScanEventPhase>,
    ) -> Result<
        PassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        PassiveScanControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let current = self;
        let graph = match current.controller.passive_scan_resources.begin_event() {
            Ok(graph) => graph,
            Err(error) => {
                return Err(PassiveScanControllerInitialPreparationFailure::Rejected {
                    current,
                    error: PassiveScanControllerPreparationError::Runtime(error),
                });
            }
        };
        let (controller, now) = current.into_parts();
        controller
            .begin_passive_scan_preparation_time(
                PassiveScanControllerPreparationPhase::AlwaysAwakeTiming {
                    graph,
                    channel: crate::le::scanning::passive::lower_primary_channel(channel),
                    parameters,
                    previous_phase,
                    now,
                },
            )
            .map_err(PassiveScanControllerInitialPreparationFailure::FailStop)
    }

    /// Begin source-ordered initial transmitter preparation.
    ///
    /// The private current is retained while the always-awake BLE-PHY owner
    /// obtains a later source-ordered post-enable timing instant. Only that
    /// ordered pair can form the candidate before admission and sequence
    /// requests.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete TX retry owner"
    )]
    pub(crate) fn begin_dtm_transmitter_first_item(
        self,
        pattern: crate::le::dtm::DtmPayloadPattern,
        length: crate::le::dtm::DtmPayloadLength,
        channel: crate::le::dtm::DtmChannel,
        phy: crate::le::dtm::DtmPhy,
        requested_interval_micros: u16,
    ) -> Result<
        DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let graph = match self.controller.dtm_resources.begin_session_epoch() {
            Ok(graph) => graph,
            Err(crate::le::dtm::DtmRuntimeSessionBeginError::SessionActive) => {
                return Err(DtmControllerInitialPreparationFailure::SessionActive(self));
            }
        };
        let owner = crate::le::dtm::DtmTxGraphPrepare::prepare_dtm_tx_packet(
            graph.into_graph(),
            pattern,
            length,
        );
        let link_state = self
            .controller
            .new_dtm_link_state_reset(crate::le::dtm::DtmRole::Transmitter);
        let (controller, now) = self.into_parts();
        controller
            .begin_dtm_always_awake_timing(
                DtmControllerPreparationPhase::TransmitterFirstAlwaysAwakeTiming {
                    owner,
                    link_state,
                    channel,
                    phy,
                    requested_interval_micros,
                    now,
                },
            )
            .map_err(DtmControllerInitialPreparationFailure::PreparationTerminal)
    }

    /// Begin source-ordered initial receiver preparation.
    ///
    /// The current retained by this state precedes a private source-owned
    /// post-enable timing request. Admission and sequence then remain private affine
    /// requests on the same Controller.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete RX retry owner"
    )]
    pub(crate) fn begin_dtm_receiver_first_item(
        self,
        channel: crate::le::dtm::DtmChannel,
        phy: crate::le::dtm::DtmPhy,
    ) -> Result<
        DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let graph = match self.controller.dtm_resources.begin_session_epoch() {
            Ok(graph) => graph,
            Err(crate::le::dtm::DtmRuntimeSessionBeginError::SessionActive) => {
                return Err(DtmControllerInitialPreparationFailure::SessionActive(self));
            }
        };
        let owner = crate::le::dtm::DtmReceiverCpuOwned::new(graph.into_graph());
        let link_state = self
            .controller
            .new_dtm_link_state_reset(crate::le::dtm::DtmRole::Receiver);
        let (controller, now) = self.into_parts();
        controller
            .begin_dtm_always_awake_timing(
                DtmControllerPreparationPhase::ReceiverFirstAlwaysAwakeTiming {
                    owner,
                    link_state,
                    channel,
                    phy,
                    now,
                },
            )
            .map_err(DtmControllerInitialPreparationFailure::PreparationTerminal)
    }

    /// Reserve and begin source-ordered recurring transmitter preparation.
    ///
    /// The recurring window is reserved before its private sequence request.
    /// This path has no post-enable timing phase in the reviewed vendor flow.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains the Controller and complete active TX owner"
    )]
    pub fn begin_dtm_transmitter_recurring_item(
        self,
        owner: crate::le::dtm::DtmActiveTransmitterCpuOwned,
    ) -> Result<
        DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let (mut controller, now) = self.into_parts();
        let staged = match controller
            .runtime
            .stage_dtm_transmitter_recurring_item(owner, now)
        {
            Ok(staged) => staged,
            Err(failure) => {
                return Err(DtmControllerPreparationTerminal {
                    controller: ControllerSchedulerEpochRetained { controller },
                    outcome: DtmControllerPreparationOutcome::TransmitterRecurring(Err(failure)),
                });
            }
        };
        let pre_sequence = match controller
            .runtime
            .reserve_dtm_transmitter_recurring_item(staged)
        {
            Ok(pre_sequence) => pre_sequence,
            Err(failure) => {
                return Err(DtmControllerPreparationTerminal {
                    controller: ControllerSchedulerEpochRetained { controller },
                    outcome: DtmControllerPreparationOutcome::TransmitterRecurring(Err(failure)),
                });
            }
        };
        controller.begin_dtm_preparation_time(
            DtmControllerPreparationPhase::TransmitterRecurringSequence(pre_sequence),
        )
    }
}

/// Failed stable publication retaining the Controller, storage and role runtimes.
#[must_use = "failed ISR publication returns every affine owner for inspection or retry"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerInterruptOwnerPublicationFailure<
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> where
    S: InterruptOwnerStorage,
{
    controller: ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    storage: S,
    dtm_resources: crate::le::dtm::DtmRuntimeResources,
    legacy_advertising_resources: crate::le::advertising::LegacyAdvertisingRuntimeResources,
    passive_scan_resources: crate::le::scanning::PassiveScanRuntimeResources,
    peripheral_connection_resources: crate::le::peripheral::PeripheralConnectionRuntimeResources,
    legacy_connectable_advertising_resources:
        crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
    error: S::Error,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(&self) -> crate::ble_phy::BlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::common_phy_state::PhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Conditional runtime-control branch retained across the timer start.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.timer.runtime_control_observation()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerInterruptOwnersPublished<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
where
    S: ModemLpTimerSoftwareOwnerStorage,
{
    /// Borrow the published hardware graph as disjoint interrupt and task endpoints.
    ///
    /// The caller must retain this owner in stable storage for the complete
    /// routed lifetime. HCI remains a separate protocol resource until the
    /// post-publication binding transition.
    pub(crate) fn split_hardware_runtime<'runtime>(
        &'runtime mut self,
    ) -> ControllerPublishedHardwareRuntimeEndpoints<
        'runtime,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    > {
        let Self {
            initialized,
            _storage,
            post_unlink_mailbox,
            scheduler_epoch,
            dtm_resources,
            legacy_advertising_resources,
            passive_scan_resources,
            peripheral_connection_resources,
            legacy_connectable_advertising_resources,
            ..
        } = self;
        let direction_finding_workspace = initialized.direction_finding_workspace_link();
        let (
            ControllerRuntimeEndpoints {
                interrupt,
                task,
                modem_timer,
            },
            ble_phy_timing,
        ) = initialized.split_runtime();
        let interrupt = ControllerPublishedInterruptService {
            storage: _storage,
            runtime: interrupt,
            mailbox: post_unlink_mailbox,
        };
        let task = ControllerPublishedTaskService {
            storage: _storage,
            runtime: task,
            mailbox: post_unlink_mailbox,
            dtm_resources,
            legacy_advertising_resources,
            passive_scan_resources,
            peripheral_connection_resources,
            legacy_connectable_advertising_resources,
            direction_finding_workspace,
            ble_phy_timing,
            scheduler_epoch,
        };
        let modem_timer = ControllerModemTimerTask::new(_storage, modem_timer);
        ControllerPublishedHardwareRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
        }
    }

    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(&self) -> crate::ble_phy::BlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::common_phy_state::PhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Conditional runtime-control branch retained across publication.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Normalize the hardware timestamp captured beside one received LE 1M PDU.
    ///
    /// This uses the persistent scheduler epoch and the calibration retained by
    /// the initialized BLE PHY storage. It performs no MMIO, does not sample
    /// `now()`, and does not advance the scheduler epoch.
    pub fn normalize_le_1m_packet_start(
        &mut self,
        packet: &oer_esp32s31_bluetooth_memory::LeReceivedPdu,
    ) -> Result<crate::le::peripheral::Le1MPacketStartTiming, LePacketStartTimingError> {
        let epoch =
            (*self.scheduler_epoch).ok_or(LePacketStartTimingError::SchedulerEpochUnavailable)?;
        Ok(self
            .ble_phy_timing
            .complete_le_1m_packet_start(epoch, packet.captured_time()))
    }

    /// Move this exact task service into its initialized scheduler epoch.
    ///
    /// This transition performs no MMIO, exposes no epoch image and cannot
    /// initialize missing state.
    pub fn retain_scheduler_epoch(
        self,
    ) -> Result<
        ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        ControllerSchedulerEpochUnavailable<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        if self.scheduler_epoch.is_none() {
            return Err(ControllerSchedulerEpochUnavailable { controller: self });
        }
        Ok(ControllerSchedulerEpochRetained { controller: self })
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
        AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
        AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        if self.scheduler_epoch.is_some() {
            return Err(AlwaysAwakePostEnableTimeBeginFailure {
                controller: self,
                error: AlwaysAwakePostEnableTimeBeginError::AlreadyInitialized,
            });
        }
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                return Err(AlwaysAwakePostEnableTimeBeginFailure {
                    controller: self,
                    error: error.into(),
                });
            }
        };
        Ok(AlwaysAwakePostEnableTimePending {
            core: ControllerTimePendingCore::new(self, request),
        })
    }

    /// Perform one bounded observation of an abandoned post-enable request.
    ///
    /// `Waiting` requires one later call. A completed orphan is discarded and
    /// never becomes a sample or readiness instant for a subsequent operation.
    pub fn drain_abandoned_always_awake_post_enable_time(
        &mut self,
    ) -> Result<AlwaysAwakePostEnableTimeOrphanDrainStep, AlwaysAwakePostEnableTimeError> {
        match drain_controller_time_orphan(self) {
            Ok(ControllerTimePendingOrphanStep::Idle) => {
                Ok(AlwaysAwakePostEnableTimeOrphanDrainStep::Idle)
            }
            Ok(ControllerTimePendingOrphanStep::Waiting) => {
                Ok(AlwaysAwakePostEnableTimeOrphanDrainStep::Waiting)
            }
            Ok(ControllerTimePendingOrphanStep::Drained) => {
                Ok(AlwaysAwakePostEnableTimeOrphanDrainStep::Drained)
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
        merged: crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmTransmitterEvent,
            crate::scheduler::DtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        (
            oer_esp32s31_bluetooth_memory::DtmMemoryGraphCpuOwned,
            crate::le::dtm::DtmPayloadPattern,
            crate::le::dtm::DtmPayloadLength,
        ),
        crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmTransmitterEvent,
            crate::scheduler::DtmInitialSchedulerItemPhase,
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
        merged: crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmReceiverEvent,
            crate::scheduler::DtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        crate::le::dtm::DtmReceiverCpuOwned,
        crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmReceiverEvent,
            crate::scheduler::DtmInitialSchedulerItemPhase,
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
        merged: crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmTransmitterEvent,
            crate::scheduler::DtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        crate::le::dtm::DtmActiveTransmitterCpuOwned,
        crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmTransmitterEvent,
            crate::scheduler::DtmRecurringSchedulerItemPhase,
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
        merged: crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmReceiverEvent,
            crate::scheduler::DtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        crate::le::dtm::DtmActiveReceiverCpuOwned,
        crate::scheduler::DtmEmptySchedulerMergePrepared<
            crate::le::dtm::DtmReceiverEvent,
            crate::scheduler::DtmRecurringSchedulerItemPhase,
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
        merged: crate::scheduler::DtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        crate::scheduler::DtmSchedulerHeadPublished<Role>,
        crate::scheduler::DtmSchedulerHeadPublicationFailure<Role, Phase>,
    >
    where
        Phase: crate::le::dtm::DtmSchedulerItemPhase<Role>,
    {
        self.runtime.publish_dtm_scheduler_head(merged)
    }

    /// Publish the first advertising item through the same exclusive head edge.
    #[expect(
        clippy::result_large_err,
        reason = "pre-MMIO rejection returns the complete advertising graph"
    )]
    pub fn publish_legacy_advertising_scheduler_head<'a>(
        &mut self,
        merged: crate::scheduler::LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<
        crate::scheduler::LegacyAdvertisingSchedulerHeadPublished<'a>,
        crate::scheduler::LegacyAdvertisingSchedulerHeadPublicationFailure<'a>,
    > {
        self.runtime
            .publish_legacy_advertising_scheduler_head(merged)
    }

    /// Publish the first passive scanner item through the exclusive head edge.
    pub(crate) fn publish_passive_scan_scheduler_head(
        &mut self,
        merged: crate::scheduler::PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<
        crate::scheduler::PassiveScanSchedulerHeadPublished,
        crate::scheduler::PassiveScanSchedulerHeadPublicationFailure,
    > {
        self.runtime.publish_passive_scan_scheduler_head(merged)
    }
}

#[cfg(target_arch = "riscv32")]
impl<S> ControllerPublishedInterruptService<'_, S> {
    /// Borrow the exact stable-storage publication backing this interrupt
    /// service.
    ///
    /// Platform integration uses this only after the complete service and its
    /// executor notifications have reached stable storage, then binds the CPU
    /// routes as the final activation edge. The borrow cannot duplicate or
    /// recover the affine publication owner.
    pub const fn storage(&self) -> &S {
        self.storage
    }

    /// Service, durably publish and route one primary source-124 epoch through
    /// the Controller-owned post-unlink mailbox.
    ///
    /// Capture/acknowledge, both ordinary cell publications and the capacity-one
    /// mailbox transition are serialized by one critical section. An armed
    /// mailbox stores exactly the first eligible event; a full mailbox returns
    /// the newer event without replacing the retained one.
    pub fn service_primary_interrupt(
        &self,
    ) -> Result<crate::le::dtm::PrimarySerializedServiceStep, S::Error>
    where
        S: SharedInterruptDispatchStorage,
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
    ) -> Result<ModemLpTimerPublishedInterruptStep, S::Error>
    where
        S: ModemLpTimerInterruptDispatchStorage,
    {
        let step = self.storage.service_modem_lp_timer_interrupt()?;
        Ok(step.publish(self.runtime.modem_lp_timer_worker_wake()))
    }

    /// Service one opaque default-profile NRT source-133 epoch.
    ///
    /// The reviewed default path intentionally publishes no scheduler or
    /// Link-Layer work and keeps the shared owner in stable platform storage.
    pub fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, S::Error>
    where
        S: SharedInterruptDispatchStorage,
    {
        self.storage.service_nrt_default_interrupt()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerInterruptOwnerPublicationFailure<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
where
    S: InterruptOwnerStorage,
{
    /// Inspect the exact platform rejection.
    pub const fn error(&self) -> &S::Error {
        &self.error
    }

    /// Recover the complete pre-publication Controller, storage and role runtimes.
    pub fn into_parts(
        self,
    ) -> (
        ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        S,
        crate::le::dtm::DtmRuntimeResources,
        crate::le::advertising::LegacyAdvertisingRuntimeResources,
        crate::le::scanning::PassiveScanRuntimeResources,
        crate::le::peripheral::PeripheralConnectionRuntimeResources,
        crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
        S::Error,
    ) {
        (
            self.controller,
            self.storage,
            self.dtm_resources,
            self.legacy_advertising_resources,
            self.passive_scan_resources,
            self.peripheral_connection_resources,
            self.legacy_connectable_advertising_resources,
            self.error,
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Inspect the BLE PHY input retained by this exact powered epoch.
    pub const fn ble_phy_report(&self) -> crate::ble_phy::BlePhyInitializationReport {
        self.initialized.report()
    }

    /// Inspect the preceding finite BTBB transition.
    pub const fn baseband_report(&self) -> crate::baseband::BasebandInitializationReport {
        self.initialized.baseband_report()
    }

    /// Inspect the complete common-PHY transition.
    pub const fn phy_report(&self) -> crate::common_phy_state::PhyInitializationReport {
        self.initialized.phy_report()
    }

    /// Conditional runtime-control branch retained by the ISR-ready timer.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Atomically publish both owners in caller-selected stable ISR storage.
    ///
    /// Rejection occurs before publication and returns this exact state plus
    /// the storage capability and unmodified role runtimes. Success still leaves
    /// every CPU route inactive.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return every affine powered owner"
    )]
    pub fn publish_interrupt_owners<S>(
        self,
        storage: S,
        dtm_resources: crate::le::dtm::DtmRuntimeResources,
        legacy_advertising_resources: crate::le::advertising::LegacyAdvertisingRuntimeResources,
        passive_scan_resources: crate::le::scanning::PassiveScanRuntimeResources,
        peripheral_connection_resources: crate::le::peripheral::PeripheralConnectionRuntimeResources,
        legacy_connectable_advertising_resources:
            crate::le::advertising::LegacyConnectableAdvertisingRuntimeResources,
    ) -> Result<
        ControllerInterruptOwnersPublished<
            P,
            S::Published,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
        ControllerInterruptOwnerPublicationFailure<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    >
    where
        S: InterruptOwnerStorage,
    {
        let Self {
            initialized,
            _interrupts: interrupts,
            _timer: timer,
            runtime_control,
        } = self;
        match storage.publish(interrupts, timer) {
            Ok(published) => Ok(ControllerInterruptOwnersPublished {
                initialized,
                _storage: published,
                post_unlink_mailbox: DtmPostUnlinkMailbox::new(),
                runtime_control,
                scheduler_epoch: None,
                dtm_resources,
                legacy_advertising_resources,
                passive_scan_resources,
                peripheral_connection_resources,
                legacy_connectable_advertising_resources,
            }),
            Err((error, storage, interrupts, timer)) => {
                Err(ControllerInterruptOwnerPublicationFailure {
                    controller: ControllerInterruptOwnersReady {
                        initialized,
                        _interrupts: interrupts,
                        _timer: timer,
                        runtime_control,
                    },
                    storage,
                    dtm_resources,
                    legacy_advertising_resources,
                    passive_scan_resources,
                    peripheral_connection_resources,
                    legacy_connectable_advertising_resources,
                    error,
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Transfer both disjoint register owners into their pre-route states.
    ///
    /// This is an ownership-only transition. It performs no MMIO and does not
    /// claim stable placement or a live interrupt epoch.
    pub fn stage_interrupt_owners(
        self,
    ) -> ControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        let Self {
            initialized,
            _interrupt_output: interrupt_output,
            timer,
        } = self;
        let runtime_control = timer.runtime_control_observation();
        ControllerInterruptOwnersReady {
            initialized,
            _interrupts: interrupt_output.stage_for_cpu_routes(),
            _timer: timer.stage_for_interrupt(),
            runtime_control,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Prepare Controller IRQ output and then start the runtime timer once.
    ///
    /// The consuming BLE-PHY state proves that controller HAL, scheduler,
    /// low-power hardware, common PHY, BTBB, BLE PHY initialization and public
    /// Controller-address publication all belong to this epoch. CPU routes
    /// remain inaccessible, so the lower unsafe interrupt prerequisite is
    /// discharged here and never exported.
    #[allow(
        unsafe_code,
        reason = "the complete Controller typestate proves the HAL interrupt prerequisites"
    )]
    pub fn prepare_controller_output_and_start_runtime_timer(
        mut self,
    ) -> ControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
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

        ControllerOutputTimerStarted {
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
mod tests;
