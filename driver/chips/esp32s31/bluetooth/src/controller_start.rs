//! Controller-output and runtime-timer activation after BLE PHY initialization.

#[cfg(target_arch = "riscv32")]
mod scheduler_service;

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothInterruptOutputPreparedOwner,
    BluetoothInterruptRegistersOwner, BluetoothLowPowerRuntimeControlObservation,
    BluetoothModemLpTimerCounterStartedOwner, BluetoothModemLpTimerInterruptReadyOwner,
    BluetoothModemLpTimerSoftwarePendingOwner, BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothSchedulerSoftwareListRemovalJoin,
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
    BluetoothDtmPostUnlinkTake, BluetoothLegacyAdvertisingPostUnlinkRearm,
    BluetoothLegacyAdvertisingPostUnlinkTake, BluetoothPassiveScanPostUnlinkRearm,
    BluetoothPassiveScanPostUnlinkTake, BluetoothPeripheralConnectionPostUnlinkRearm,
    BluetoothPeripheralConnectionPostUnlinkTake,
};
#[cfg(target_arch = "riscv32")]
use crate::modem_lp_timer_queue::{
    BluetoothModemLpTimerExpirationState, BluetoothModemLpTimerSoftwareState,
    BluetoothModemLpTimerSoftwareStateStep,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::{
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin,
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck,
};
#[cfg(target_arch = "riscv32")]
use crate::{
    BluetoothControllerBlePhyEngineInitialized, BluetoothControllerInterruptRuntime,
    BluetoothControllerModemTimerRuntime, BluetoothControllerPoweredTaskRuntime,
    BluetoothControllerRuntimeEndpoints, BluetoothModemLpTimerEventCell,
    BluetoothModemLpTimerEventPublication, BluetoothModemLpTimerExpiration,
    BluetoothModemLpTimerPublishedInterruptStep, BluetoothModemLpTimerStableInterruptStep,
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
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) initialized:
        BluetoothControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
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
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized:
        BluetoothControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
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

    /// Recheck the complete scheduler software-list removal predicate while
    /// the stable interrupt-register owner remains in platform storage.
    ///
    /// Failure returns the unchanged affine empty-head proof.
    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut BluetoothControllerHal<'_>,
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
/// directly to [`crate::BluetoothDtmSchedulerRunning`].
#[must_use = "a failed DTM scheduler start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmSchedulerStartFailure<Role, E> {
    error: E,
    head: crate::BluetoothDtmSchedulerHeadPublished<Role>,
}

/// Failed advertising scheduler start before the synchronous run suffix began.
#[must_use = "a failed advertising scheduler start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothLegacyAdvertisingSchedulerStartFailure<'a, E> {
    error: E,
    head: crate::BluetoothLegacyAdvertisingSchedulerHeadPublished<'a>,
}

/// Failed scanner scheduler start before the synchronous RUN suffix began.
#[must_use = "a failed scanner start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPassiveScanSchedulerStartFailure<E> {
    error: E,
    head: crate::BluetoothPassiveScanSchedulerHeadPublished,
}

/// Failed connection scheduler start before the synchronous RUN suffix began.
#[must_use = "a failed connection start still owns its published graph"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerStartFailure<E> {
    error: E,
    head: crate::BluetoothPeripheralConnectionSchedulerHeadPublished,
}

#[cfg(target_arch = "riscv32")]
impl<E> BluetoothPeripheralConnectionSchedulerStartFailure<E> {
    /// Inspect the stable interrupt-storage rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged published connection graph.
    pub fn into_parts(
        self,
    ) -> (
        E,
        crate::BluetoothPeripheralConnectionSchedulerHeadPublished,
    ) {
        (self.error, self.head)
    }
}

#[cfg(target_arch = "riscv32")]
impl<E> BluetoothPassiveScanSchedulerStartFailure<E> {
    /// Inspect the exact stable interrupt-storage rejection.
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Recover the error and unchanged published scanner graph.
    pub fn into_parts(self) -> (E, crate::BluetoothPassiveScanSchedulerHeadPublished) {
        (self.error, self.head)
    }
}

#[cfg(target_arch = "riscv32")]
impl<'a, E> BluetoothLegacyAdvertisingSchedulerStartFailure<'a, E> {
    pub const fn error(&self) -> &E {
        &self.error
    }

    pub fn into_parts(
        self,
    ) -> (
        E,
        crate::BluetoothLegacyAdvertisingSchedulerHeadPublished<'a>,
    ) {
        (self.error, self.head)
    }
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
    /// An interrupt-derived scheduler observation was not ready.
    PublishedPending {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    },
    /// A direct task-side scheduler observation was not ready.
    DirectPending {
        /// Already-unlinked graph re-armed before leaving the serialization boundary.
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    },
    /// The stable interrupt-register owner was unavailable for direct recheck.
    RecheckUnavailable {
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
    /// An internal mailbox invariant rejected re-arm after direct recheck.
    RecheckRearmMismatch {
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
    },
    /// The graph belongs to another Controller scheduler epoch.
    SchedulerIdentityMismatch {
        /// Unchanged already-unlinked graph.
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        /// Exact classified event which was not consumed by the mismatched graph.
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    /// The graph does not belong to the scheduler epoch performing a direct recheck.
    DirectSchedulerIdentityMismatch {
        unlinked: crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
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

/// Result of atomically unlinking advertising and arming the return mailbox.
#[must_use = "retain the empty-head graph or armed advertising owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingPostUnlinkArmStep<'a> {
    MailboxBusy(crate::BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>),
    MailboxIdentityExhausted(
        crate::BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>,
    ),
    GenerationExhausted(crate::BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>),
    SchedulerIdentityMismatch(
        crate::BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>,
    ),
    MailboxCommitMismatch(crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>),
    Armed(crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>),
}

/// Controller result of consuming one advertising post-unlink event pair.
#[must_use = "every outcome retains the advertising graph"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep<'a> {
    MailboxAffinityMismatch(crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>),
    Fault {
        unlinked: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
        fault: crate::BluetoothPrimaryControllerFault,
    },
    NoSchedulerWork {
        awaiting: crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    PublishedPending {
        awaiting: crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>,
    },
    DirectPending {
        awaiting: crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>,
    },
    RecheckUnavailable {
        awaiting: crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>,
    },
    NoSchedulerWorkRearmMismatch {
        unlinked: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    PendingRearmMismatch {
        unlinked: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
    },
    RecheckRearmMismatch {
        unlinked: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
    },
    SchedulerIdentityMismatch {
        unlinked: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    DirectSchedulerIdentityMismatch {
        unlinked: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
    },
    Ready {
        ready: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>,
    },
}

/// Result of atomically unlinking a scanner item and arming the return mailbox.
#[must_use = "retain the empty-head scanner graph or armed owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPassiveScanPostUnlinkArmStep {
    MailboxBusy(crate::BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
    MailboxIdentityExhausted(crate::BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
    GenerationExhausted(crate::BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
    SchedulerIdentityMismatch(crate::BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
    MailboxCommitMismatch(crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked),
    Armed(crate::BluetoothPassiveScanPostUnlinkAwaiting),
}

/// Controller result of consuming one scanner post-unlink event pair.
#[must_use = "every outcome retains the scanner graph"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPassiveScanSoftwareListRemovalPublishedStep {
    MailboxAffinityMismatch(crate::BluetoothPassiveScanPostUnlinkAwaiting),
    Fault {
        unlinked: crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked,
        fault: crate::BluetoothPrimaryControllerFault,
    },
    NoSchedulerWork {
        awaiting: crate::BluetoothPassiveScanPostUnlinkAwaiting,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    PublishedPending {
        awaiting: crate::BluetoothPassiveScanPostUnlinkAwaiting,
    },
    DirectPending {
        awaiting: crate::BluetoothPassiveScanPostUnlinkAwaiting,
    },
    RecheckUnavailable {
        awaiting: crate::BluetoothPassiveScanPostUnlinkAwaiting,
    },
    NoSchedulerWorkRearmMismatch {
        unlinked: crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    PendingRearmMismatch {
        unlinked: crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked,
    },
    RecheckRearmMismatch {
        unlinked: crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked,
    },
    SchedulerIdentityMismatch {
        unlinked: crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    DirectSchedulerIdentityMismatch {
        unlinked: crate::BluetoothPassiveScanSchedulerSoftwareListUnlinked,
    },
    Ready {
        ready: crate::BluetoothPassiveScanSchedulerSoftwareListRemovalReady,
    },
}

/// Result of atomically unlinking a connection item and arming its return mailbox.
#[must_use = "retain the empty-head connection graph or armed owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionPostUnlinkArmStep {
    MailboxBusy(crate::BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved),
    MailboxIdentityExhausted(
        crate::BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved,
    ),
    GenerationExhausted(crate::BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved),
    SchedulerIdentityMismatch(
        crate::BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved,
    ),
    MailboxCommitMismatch(crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked),
    Armed(crate::BluetoothPeripheralConnectionPostUnlinkAwaiting),
}

/// Controller result of consuming one connection post-unlink event pair.
#[must_use = "every outcome retains the connection graph"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep {
    MailboxAffinityMismatch(crate::BluetoothPeripheralConnectionPostUnlinkAwaiting),
    Fault {
        unlinked: crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
        fault: crate::BluetoothPrimaryControllerFault,
    },
    NoSchedulerWork {
        awaiting: crate::BluetoothPeripheralConnectionPostUnlinkAwaiting,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    PublishedPending {
        awaiting: crate::BluetoothPeripheralConnectionPostUnlinkAwaiting,
    },
    DirectPending {
        awaiting: crate::BluetoothPeripheralConnectionPostUnlinkAwaiting,
    },
    RecheckUnavailable {
        awaiting: crate::BluetoothPeripheralConnectionPostUnlinkAwaiting,
    },
    NoSchedulerWorkRearmMismatch {
        unlinked: crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
        epoch: crate::BluetoothPrimaryNoSchedulerWork,
    },
    PendingRearmMismatch {
        unlinked: crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
    },
    RecheckRearmMismatch {
        unlinked: crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
    },
    SchedulerIdentityMismatch {
        unlinked: crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    DirectSchedulerIdentityMismatch {
        unlinked: crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
    },
    Ready {
        ready: crate::BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady,
    },
}

/// Stable state retained inside the disjoint source-127 task endpoint.
#[cfg(target_arch = "riscv32")]
enum BluetoothControllerModemTimerTaskPhase {
    Idle,
    Work(BluetoothModemLpTimerSoftwareState),
    Expiration(BluetoothModemLpTimerExpirationState),
    Rearm(BluetoothModemLpTimerInterruptReadyOwner),
}

/// Borrowed readiness class for the source-127 task endpoint.
///
/// The value carries no timer owner and may be held by an executor wait future.
/// The affine queue, epoch and HAL phase remain inside
/// [`BluetoothControllerModemTimerTask`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerModemTimerReadinessClass {
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
pub struct BluetoothControllerModemTimerReadiness<'task> {
    class: BluetoothControllerModemTimerReadinessClass,
    worker_wake: &'task crate::BluetoothModemLpTimerWorkerWakeCell,
    events: &'task BluetoothModemLpTimerEventCell,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothControllerModemTimerReadiness<'_> {
    /// Exact state an executor wait is observing.
    pub const fn class(&self) -> BluetoothControllerModemTimerReadinessClass {
        self.class
    }

    /// Recheck readiness after registering the executor's own waker.
    ///
    /// This operation is borrowed and value-only. It neither acquires stable
    /// ownership nor advances queue or register state.
    pub fn is_ready(&self) -> bool {
        match self.class {
            BluetoothControllerModemTimerReadinessClass::Interrupt => self.worker_wake.is_pending(),
            BluetoothControllerModemTimerReadinessClass::Step
            | BluetoothControllerModemTimerReadinessClass::Rearm => true,
            BluetoothControllerModemTimerReadinessClass::EventCapacity => !self.events.is_pending(),
        }
    }
}

/// Result of acquiring one durable source-127 software-pending owner.
#[must_use = "a begin result must be handled without losing stable readiness"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerModemTimerBegin<E> {
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
pub enum BluetoothControllerModemTimerStep {
    /// No software owner has been acquired.
    Idle,
    /// One expiration now owns the durable publication edge.
    ExpirationPending(BluetoothModemLpTimerExpiration),
    /// The expiration was published and later work remains retained.
    Published(BluetoothModemLpTimerEventPublication),
    /// The expiration cell is occupied and the unchanged publication remains retained.
    Backpressured(BluetoothModemLpTimerExpiration),
    /// Immediate compare disposition requires a later fresh finite recheck.
    Recheck,
    /// Queue processing produced a fully rearmed owner retained inside the endpoint.
    RearmPending,
}

/// Result of restoring the fully rearmed owner to stable ISR storage.
#[must_use = "a rejected rearm remains retained by the timer task"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerModemTimerRearm<E> {
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
pub struct BluetoothControllerModemTimerTask<'runtime, S, const CAPACITY: usize> {
    storage: &'runtime S,
    runtime: BluetoothControllerModemTimerRuntime<'runtime, CAPACITY>,
    phase: BluetoothControllerModemTimerTaskPhase,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const CAPACITY: usize> BluetoothControllerModemTimerTask<'runtime, S, CAPACITY>
where
    S: BluetoothModemLpTimerSoftwareOwnerStorage,
{
    fn new(
        storage: &'runtime S,
        runtime: BluetoothControllerModemTimerRuntime<'runtime, CAPACITY>,
    ) -> Self {
        Self {
            storage,
            runtime,
            phase: BluetoothControllerModemTimerTaskPhase::Idle,
        }
    }

    /// Borrow the current owner-free readiness predicate.
    pub fn readiness(&self) -> BluetoothControllerModemTimerReadiness<'_> {
        let class = match &self.phase {
            BluetoothControllerModemTimerTaskPhase::Idle => {
                BluetoothControllerModemTimerReadinessClass::Interrupt
            }
            BluetoothControllerModemTimerTaskPhase::Work(_) => {
                BluetoothControllerModemTimerReadinessClass::Step
            }
            BluetoothControllerModemTimerTaskPhase::Expiration(_) => {
                BluetoothControllerModemTimerReadinessClass::EventCapacity
            }
            BluetoothControllerModemTimerTaskPhase::Rearm(_) => {
                BluetoothControllerModemTimerReadinessClass::Rearm
            }
        };
        BluetoothControllerModemTimerReadiness {
            class,
            worker_wake: self.runtime.worker_wake(),
            events: self.runtime.events(),
        }
    }

    /// Acquire exactly one software-pending owner after borrowed readiness.
    pub fn begin(&mut self) -> BluetoothControllerModemTimerBegin<S::TakeError> {
        if !matches!(self.phase, BluetoothControllerModemTimerTaskPhase::Idle) {
            return BluetoothControllerModemTimerBegin::AlreadyActive;
        }
        if !self.runtime.worker_wake().is_pending() {
            return BluetoothControllerModemTimerBegin::NotReady;
        }
        match self.storage.take_modem_lp_timer_software_pending() {
            Ok(owner) => {
                self.runtime.worker_wake.take();
                self.phase = BluetoothControllerModemTimerTaskPhase::Work(
                    BluetoothModemLpTimerSoftwareState::begin(owner, self.runtime.epoch),
                );
                BluetoothControllerModemTimerBegin::Started
            }
            Err(error) => BluetoothControllerModemTimerBegin::StorageRejected(error),
        }
    }

    /// Advance exactly one queue, publication or compare transition.
    pub fn step(&mut self) -> BluetoothControllerModemTimerStep {
        let phase = core::mem::replace(
            &mut self.phase,
            BluetoothControllerModemTimerTaskPhase::Idle,
        );
        match phase {
            BluetoothControllerModemTimerTaskPhase::Idle => BluetoothControllerModemTimerStep::Idle,
            BluetoothControllerModemTimerTaskPhase::Work(work) => {
                match work.step(self.runtime.queue, self.runtime.epoch) {
                    BluetoothModemLpTimerSoftwareStateStep::Expiration(pending) => {
                        let event = pending.event();
                        self.phase = BluetoothControllerModemTimerTaskPhase::Expiration(pending);
                        BluetoothControllerModemTimerStep::ExpirationPending(event)
                    }
                    BluetoothModemLpTimerSoftwareStateStep::Recheck(work) => {
                        self.phase = BluetoothControllerModemTimerTaskPhase::Work(work);
                        BluetoothControllerModemTimerStep::Recheck
                    }
                    BluetoothModemLpTimerSoftwareStateStep::Rearmed(owner) => {
                        self.phase = BluetoothControllerModemTimerTaskPhase::Rearm(owner);
                        BluetoothControllerModemTimerStep::RearmPending
                    }
                }
            }
            BluetoothControllerModemTimerTaskPhase::Expiration(pending) => {
                let event = pending.event();
                match pending.publish(self.runtime.events()) {
                    Ok((work, publication)) => {
                        self.phase = BluetoothControllerModemTimerTaskPhase::Work(work);
                        BluetoothControllerModemTimerStep::Published(publication)
                    }
                    Err(pending) => {
                        self.phase = BluetoothControllerModemTimerTaskPhase::Expiration(pending);
                        BluetoothControllerModemTimerStep::Backpressured(event)
                    }
                }
            }
            BluetoothControllerModemTimerTaskPhase::Rearm(owner) => {
                self.phase = BluetoothControllerModemTimerTaskPhase::Rearm(owner);
                BluetoothControllerModemTimerStep::RearmPending
            }
        }
    }

    /// Restore one fully rearmed owner to stable source-127 interrupt storage.
    pub fn rearm(&mut self) -> BluetoothControllerModemTimerRearm<S::RestoreError> {
        let phase = core::mem::replace(
            &mut self.phase,
            BluetoothControllerModemTimerTaskPhase::Idle,
        );
        let BluetoothControllerModemTimerTaskPhase::Rearm(owner) = phase else {
            self.phase = phase;
            return BluetoothControllerModemTimerRearm::NotReady;
        };
        self.runtime.worker_wake.take();
        match self.storage.restore_modem_lp_timer_ready(owner) {
            Ok(()) => BluetoothControllerModemTimerRearm::Rearmed,
            Err((error, owner)) => {
                self.phase = BluetoothControllerModemTimerTaskPhase::Rearm(owner);
                BluetoothControllerModemTimerRearm::StorageRejected(error)
            }
        }
    }

    /// Consume one durably published expiration, if present.
    pub fn take_expiration(&mut self) -> Option<BluetoothModemLpTimerExpiration> {
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
pub struct BluetoothControllerInterruptOwnersPublished<
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    initialized:
        BluetoothControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    _storage: S,
    post_unlink_mailbox: BluetoothDtmPostUnlinkMailbox,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
    scheduler_epoch: Option<crate::BluetoothControllerSchedulerEpoch>,
    dtm_resources: crate::BluetoothDtmRuntimeResources,
    legacy_advertising_resources: crate::BluetoothLegacyAdvertisingRuntimeResources,
    passive_scan_resources: crate::BluetoothPassiveScanRuntimeResources,
    peripheral_connection_resources: crate::BluetoothPeripheralConnectionRuntimeResources,
}

/// Hardware/task endpoints prepared after stable interrupt-owner publication.
///
/// HCI is intentionally absent: protocol resources are bound only after this
/// hardware ownership graph has reached its final movable state.
#[must_use = "published hardware endpoints must remain in one live runtime epoch"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothControllerPublishedHardwareRuntimeEndpoints<
    'runtime,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    pub(crate) interrupt: BluetoothControllerPublishedInterruptService<'runtime, S>,
    pub(crate) task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    pub(crate) modem_timer: BluetoothControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedHardwareRuntimeEndpoints<
        'runtime,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
{
    pub(crate) fn bind_hci<M, const H2C: usize, const C2H: usize, const PC: usize>(
        self,
        mut hci: open_esp_radio_bluetooth_hci::LeControllerHciEndpoints<'runtime, M, H2C, C2H, PC>,
    ) -> BluetoothControllerPublishedRuntimeSplit<
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
            open_esp_radio_bluetooth_hci::LeControllerCommandReadyClaim::Ready(ready) => {
                BluetoothControllerPublishedRuntimeSplit::Ready(
                    BluetoothControllerPublishedRuntimeEndpoints {
                        interrupt,
                        task: BluetoothControllerIdleCommandTask::from_ready(ready),
                        modem_timer,
                        hci,
                    },
                )
            }
            open_esp_radio_bluetooth_hci::LeControllerCommandReadyClaim::AlreadyClaimed(task) => {
                BluetoothControllerPublishedRuntimeSplit::CommandReadyUnavailable(
                    BluetoothControllerPublishedRuntimeSplitFailure {
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
pub struct BluetoothControllerPublishedRuntimeEndpoints<
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
    pub interrupt: BluetoothControllerPublishedInterruptService<'runtime, S>,
    /// Sole idle task carrying this epoch's affine next-command authority.
    pub task: BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
    /// Disjoint source-127 queue, epoch and stable-storage task endpoint.
    pub modem_timer: BluetoothControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
    /// Disjoint Host transport and combined Controller command endpoint.
    pub hci: open_esp_radio_bluetooth_hci::LeControllerHciEndpoints<
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
pub enum BluetoothControllerPublishedRuntimeSplit<
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
        BluetoothControllerPublishedRuntimeEndpoints<
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
        BluetoothControllerPublishedRuntimeSplitFailure<
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
pub struct BluetoothControllerPublishedRuntimeSplitFailure<
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
    _interrupt: BluetoothControllerPublishedInterruptService<'runtime, S>,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _modem_timer: BluetoothControllerModemTimerTask<'runtime, S, MODEM_TIMER_CAPACITY>,
    _hci: open_esp_radio_bluetooth_hci::LeControllerHciEndpoints<
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
pub struct BluetoothControllerIdleCommandTask<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    ready: open_esp_radio_bluetooth_hci::LeControllerCommandReady<'runtime, ()>,
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
pub enum BluetoothControllerIdleCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// One command was consumed and routed; scratch storage is reusable.
    Routed {
        route:
            crate::BluetoothControllerIdleCommandRoute<'runtime, 'command, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    /// A readiness hint became stale before intake.
    Empty {
        task: BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    /// The supplied combined endpoint belongs to another HCI epoch.
    EndpointMismatch {
        task: BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    /// Packet intake failed without consuming next-command authority.
    Channel {
        task: BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        buffer: &'buffer mut [u8],
        error: open_esp_radio_bluetooth_hci::HciChannelError,
    },
    /// The oldest Host packet was data rather than a Controller command.
    NonCommand {
        task: BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>,
        frame: open_esp_radio_bluetooth_hci::HciEpochBound<
            'command,
            open_esp_radio_bluetooth_hci::HostToControllerFrame<'buffer>,
        >,
    },
}

/// One idle Controller response retaining the powered task until publication.
#[must_use = "publish the response or retain the unchanged idle transaction"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerIdleResponsePending<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    transaction: open_esp_radio_bluetooth_hci::LeControllerResponsePending<
        'runtime,
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Result of one idle response publication attempt.
#[must_use = "retain backpressure, mismatch, fault, or the returned idle command task"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerIdleResponsePublication<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// The response was durably inserted and next-command authority returned.
    Published(BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// Controller-to-Host capacity was unavailable.
    Pending(BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The supplied endpoint belongs to another HCI epoch.
    EndpointMismatch(BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// A non-capacity fault retained the exact response transaction.
    Fault {
        pending: BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        error: open_esp_radio_bluetooth_hci::HciChannelError,
    },
}

/// Idle Reset retaining both the task and exact command/order authority.
#[must_use = "complete Reset only through the matching combined endpoint"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerIdleResetBarrier<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    barrier: open_esp_radio_bluetooth_hci::LeControllerResetBarrier<
        'runtime,
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Result of applying one already-idle Reset.
#[must_use = "publish the Reset response or retain the endpoint mismatch"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothControllerIdleResetCompletion<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// Reset was applied exactly once and its response awaits publication.
    ResponsePending(BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// The endpoint belongs to another HCI epoch; Reset remains unapplied.
    EndpointMismatch(BluetoothControllerIdleResetBarrier<'runtime, S, SCHEDULER_CAPACITY>),
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn new(
        transaction: open_esp_radio_bluetooth_hci::LeControllerResponsePending<
            'runtime,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
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
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
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
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothControllerIdleResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        match self.transaction.try_publish(controller) {
            open_esp_radio_bluetooth_hci::LeControllerResponsePublication::Published(ready) => {
                BluetoothControllerIdleResponsePublication::Published(
                    BluetoothControllerIdleCommandTask::from_ready(ready),
                )
            }
            open_esp_radio_bluetooth_hci::LeControllerResponsePublication::Pending(transaction) => {
                BluetoothControllerIdleResponsePublication::Pending(Self { transaction })
            }
            open_esp_radio_bluetooth_hci::LeControllerResponsePublication::EndpointMismatch(
                transaction,
            ) => BluetoothControllerIdleResponsePublication::EndpointMismatch(Self { transaction }),
            open_esp_radio_bluetooth_hci::LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothControllerIdleResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerIdleResetBarrier<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn new(
        barrier: open_esp_radio_bluetooth_hci::LeControllerResetBarrier<
            'runtime,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
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
        controller: &mut open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothControllerIdleResetCompletion<'runtime, S, SCHEDULER_CAPACITY> {
        match controller.complete_reset_after_quiescence(self.barrier) {
            open_esp_radio_bluetooth_hci::LeControllerResetCompletion::ResponsePending(
                transaction,
            ) => BluetoothControllerIdleResetCompletion::ResponsePending(
                BluetoothControllerIdleResponsePending::new(transaction),
            ),
            open_esp_radio_bluetooth_hci::LeControllerResetCompletion::EndpointMismatch(
                barrier,
            ) => BluetoothControllerIdleResetCompletion::EndpointMismatch(Self { barrier }),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerIdleCommandTask<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) fn from_ready(
        ready: open_esp_radio_bluetooth_hci::LeControllerCommandReady<
            'runtime,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> Self {
        let (task, ready) = ready.into_parts();
        Self { task, ready }
    }

    pub(crate) const fn from_parts(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        ready: open_esp_radio_bluetooth_hci::LeControllerCommandReady<'runtime, ()>,
    ) -> Self {
        Self { task, ready }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        open_esp_radio_bluetooth_hci::LeControllerCommandReady<'runtime, ()>,
    ) {
        (self.task, self.ready)
    }

    pub(crate) fn into_ready(
        self,
    ) -> open_esp_radio_bluetooth_hci::LeControllerCommandReady<
        'runtime,
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
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
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
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
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
        controller: &mut open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            'command,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        buffer: &'buffer mut [u8],
    ) -> BluetoothControllerIdleCommandIntake<'runtime, 'command, 'buffer, S, SCHEDULER_CAPACITY>
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        match controller.try_receive_classified_command_with_buffer(self.into_ready(), buffer) {
            open_esp_radio_bluetooth_hci::LeControllerCommandIntake::Command {
                command,
                buffer,
            } => BluetoothControllerIdleCommandIntake::Routed {
                route: Self::route_idle_classified_command(controller, command),
                buffer,
            },
            open_esp_radio_bluetooth_hci::LeControllerCommandIntake::Empty { ready, buffer } => {
                BluetoothControllerIdleCommandIntake::Empty {
                    task: Self::from_ready(ready),
                    buffer,
                }
            }
            open_esp_radio_bluetooth_hci::LeControllerCommandIntake::EndpointMismatch {
                ready,
                buffer,
            } => BluetoothControllerIdleCommandIntake::EndpointMismatch {
                task: Self::from_ready(ready),
                buffer,
            },
            open_esp_radio_bluetooth_hci::LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => BluetoothControllerIdleCommandIntake::Channel {
                task: Self::from_ready(ready),
                buffer,
                error,
            },
            open_esp_radio_bluetooth_hci::LeControllerCommandIntake::NonCommand {
                ready,
                frame,
            } => BluetoothControllerIdleCommandIntake::NonCommand {
                task: Self::from_ready(ready),
                frame,
            },
        }
    }

    fn route_idle_classified_command<
        'command,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        controller: &mut open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            'command,
            M,
            H2C,
            C2H,
            PACKET,
        >,
        command: open_esp_radio_bluetooth_hci::LeControllerClassifiedCommand<
            'runtime,
            'command,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> crate::BluetoothControllerIdleCommandRoute<'runtime, 'command, S, SCHEDULER_CAPACITY>
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        match controller.route_idle_classified_command(command) {
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartReceiver(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::BluetoothDtmFirstRunner::begin(
                    task,
                    crate::BluetoothDtmDeferredStart::receiver(deferred),
                ) {
                    Ok(runner) => crate::BluetoothControllerIdleCommandRoute::Start(runner),
                    Err(failure) => crate::BluetoothControllerIdleCommandRoute::StartFailed(failure),
                }
            }
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartTransmitter(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::BluetoothDtmFirstRunner::begin(
                    task,
                    crate::BluetoothDtmDeferredStart::transmitter(deferred),
                ) {
                    Ok(runner) => crate::BluetoothControllerIdleCommandRoute::Start(runner),
                    Err(failure) => crate::BluetoothControllerIdleCommandRoute::StartFailed(failure),
                }
            }
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartLegacyAdvertising(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::BluetoothLegacyAdvertisingFirstRunner::begin(task, deferred) {
                    Ok(runner) => {
                        crate::BluetoothControllerIdleCommandRoute::StartLegacyAdvertising(runner)
                    }
                    Err(failure) => {
                        crate::BluetoothControllerIdleCommandRoute::LegacyAdvertisingStartFailed(failure)
                    }
                }
            }
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::StartLegacyScanning(
                deferred,
            ) => {
                let (task, deferred) = deferred.into_parts();
                match crate::BluetoothPassiveScanHciFirstRunner::begin(task, deferred) {
                    Ok(runner) => {
                        crate::BluetoothControllerIdleCommandRoute::StartPassiveScanning(runner)
                    }
                    Err(failure) => {
                        crate::BluetoothControllerIdleCommandRoute::PassiveScanStartFailed(failure)
                    }
                }
            }
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::ResponsePending(
                pending,
            ) => crate::BluetoothControllerIdleCommandRoute::ResponsePending(
                BluetoothControllerIdleResponsePending::new(pending),
            ),
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::ResetBarrier(
                barrier,
            ) => crate::BluetoothControllerIdleCommandRoute::ResetBarrier(
                BluetoothControllerIdleResetBarrier::new(barrier),
            ),
            open_esp_radio_bluetooth_hci::LeControllerIdleClassifiedCommandRoute::EndpointMismatch(
                command,
            ) => crate::BluetoothControllerIdleCommandRoute::EndpointMismatch(
                crate::BluetoothControllerIdleCommandMismatch::new(command),
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
    dtm_resources: &'runtime mut crate::BluetoothDtmRuntimeResources,
    legacy_advertising_resources: &'runtime mut crate::BluetoothLegacyAdvertisingRuntimeResources,
    passive_scan_resources: &'runtime mut crate::BluetoothPassiveScanRuntimeResources,
    peripheral_connection_resources:
        &'runtime mut crate::BluetoothPeripheralConnectionRuntimeResources,
    direction_finding_workspace:
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothDirectionFindingWorkspaceLink,
    ble_phy_timing: crate::ble_phy::BluetoothBlePhyTimingAuthority,
    scheduler_epoch: &'runtime mut Option<crate::BluetoothControllerSchedulerEpoch>,
}

/// Why one completed LE packet cannot yet enter scheduler time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLePacketStartTimingError {
    /// The mandatory first live controller-time sample has not established the
    /// retained scheduler epoch yet.
    SchedulerEpochUnavailable,
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
const fn controller_time_begin_error(
    error: BluetoothControllerTimeRequestError,
) -> crate::BluetoothControllerTimeAcquisitionError {
    match error {
        BluetoothControllerTimeRequestError::Busy => {
            crate::BluetoothControllerTimeAcquisitionError::Busy
        }
        BluetoothControllerTimeRequestError::OwnershipCollision => {
            crate::BluetoothControllerTimeAcquisitionError::OwnershipCollision
        }
        BluetoothControllerTimeRequestError::GenerationExhausted => {
            crate::BluetoothControllerTimeAcquisitionError::GenerationExhausted
        }
        BluetoothControllerTimeRequestError::Faulted => {
            crate::BluetoothControllerTimeAcquisitionError::Faulted
        }
    }
}

#[cfg(target_arch = "riscv32")]
const fn controller_time_event_error(
    error: BluetoothControllerTimeEventError,
) -> crate::BluetoothControllerTimeAcquisitionError {
    match error {
        BluetoothControllerTimeEventError::RequestMismatch => {
            crate::BluetoothControllerTimeAcquisitionError::RequestMismatch
        }
        BluetoothControllerTimeEventError::OwnershipLost => {
            crate::BluetoothControllerTimeAcquisitionError::OwnershipLost
        }
        BluetoothControllerTimeEventError::Faulted => {
            crate::BluetoothControllerTimeAcquisitionError::Faulted
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

/// Finite reason the first legacy-advertising graph returned to idle ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingControllerPreparationError {
    Set(crate::BluetoothLegacyAdvertisingSetError),
    ControllerTime(crate::BluetoothControllerTimeAcquisitionError),
    Runtime(crate::BluetoothLegacyAdvertisingRuntimeBeginError),
    LinkState(open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingPduError),
    TimingWindow,
    Event(crate::BluetoothLegacyAdvertisingFirstEventPreparationError),
    EmptyList(crate::BluetoothSchedulerEmptyListMergeError),
}

/// Lossless failure while rebuilding one completed advertising event.
#[must_use = "retain the scheduled or preparation owner for retry or disable"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothLegacyAdvertisingRecurringCandidateFailure {
    SchedulerEpochUnavailable(crate::BluetoothLegacyAdvertisingNextEventScheduled<'static>),
    Preparation(crate::BluetoothLegacyAdvertisingRecurringPreparationFailure<'static>),
}

/// Result of applying the recurring sequence sample and empty-list merge.
#[must_use = "retain the task and exact recurring graph outcome together"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothLegacyAdvertisingRecurringSequenceCompletion<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Prepared {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: crate::BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    },
    EventRejected {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::BluetoothLegacyAdvertisingRecurringEventPreparationFailure<'static>,
    },
    EmptyListRejected {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::BluetoothLegacyAdvertisingEmptySchedulerMergeFailure<'static>,
    },
}

/// Terminal result of one source-ordered first advertising preparation.
///
/// Rejection proves that the LL generation and exact SRAM graph were restored
/// to the same task-owned runtime. Success retains the timeline reservation and
/// exclusive-list merge until the later `HEAD` publication edge.
#[must_use = "publish the prepared item or retain the restored task owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingControllerPreparationOutcome {
    Prepared(crate::BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>),
    Rejected(BluetoothLegacyAdvertisingControllerPreparationError),
}

#[cfg(target_arch = "riscv32")]
enum BluetoothLegacyAdvertisingControllerPreparationPhase {
    AlwaysAwakeTiming {
        reset: crate::BluetoothLegacyAdvertisingLinkStateReset<'static>,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    Admission(crate::BluetoothLegacyAdvertisingFirstEventCandidate<'static>),
    Sequence(crate::BluetoothLegacyAdvertisingFirstPreSequence<'static>),
}

#[cfg(target_arch = "riscv32")]
struct BluetoothLegacyAdvertisingControllerPreparationTimeOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<BluetoothLegacyAdvertisingControllerPreparationPhase>,
    cancelled: Option<BluetoothLegacyAdvertisingControllerPreparationOutcome>,
}

/// One exact post-enable timing, admission or sequence-time request.
#[must_use = "recheck or explicitly cancel the exact advertising time request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothLegacyAdvertisingControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    core: BluetoothControllerTimePendingCore<
        BluetoothLegacyAdvertisingControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Terminal advertising preparation with the exact task service retained.
#[must_use = "the task owner and preparation outcome must be handled together"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothLegacyAdvertisingControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: BluetoothLegacyAdvertisingControllerPreparationOutcome,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothLegacyAdvertisingControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

/// Result of one bounded advertising controller-time observation.
#[must_use = "retain Pending or consume the terminal task and advertising result"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingControllerPreparationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Pending(
        BluetoothLegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Terminal(
        BluetoothLegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothLegacyAdvertisingControllerInitialPreparationFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Rejected {
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothLegacyAdvertisingControllerPreparationError,
    },
    Terminal(
        BluetoothLegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

/// Finite reason a first passive scanner event returned to idle ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPassiveScanControllerPreparationError {
    ControllerTime(crate::BluetoothControllerTimeAcquisitionError),
    Runtime(crate::BluetoothPassiveScanRuntimeBeginError),
    TimingWindow,
    Event(crate::BluetoothPassiveScanFirstEventPreparationError),
    EmptyList(crate::BluetoothSchedulerEmptyListMergeError),
}

/// Terminal result of one source-ordered first passive scanner preparation.
#[must_use = "publish the scanner item or retain the restored task owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPassiveScanControllerPreparationOutcome {
    Prepared {
        merged: crate::BluetoothPassiveScanEmptySchedulerMergePrepared,
        phase: crate::BluetoothPassiveScanEventPhase,
    },
    Rejected(BluetoothPassiveScanControllerPreparationError),
}

#[cfg(target_arch = "riscv32")]
enum BluetoothPassiveScanControllerPreparationPhase {
    AlwaysAwakeTiming {
        graph: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCpuOwned,
        channel: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanPrimaryChannel,
        parameters: open_esp_radio_bluetooth_ll::scanning::LegacyPassiveScanParameters,
        previous_phase: Option<crate::BluetoothPassiveScanEventPhase>,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    Admission {
        candidate: crate::BluetoothPassiveScanFirstEventCandidate,
        phase: crate::BluetoothPassiveScanEventPhase,
    },
    Sequence {
        admitted: crate::BluetoothPassiveScanFirstPreSequence,
        phase: crate::BluetoothPassiveScanEventPhase,
    },
}

#[cfg(target_arch = "riscv32")]
struct BluetoothPassiveScanControllerPreparationTimeOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<BluetoothPassiveScanControllerPreparationPhase>,
    cancelled: Option<BluetoothPassiveScanControllerPreparationOutcome>,
}

/// One exact post-enable timing, admission or sequence-time request.
#[must_use = "recheck or explicitly cancel the exact scanner time request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPassiveScanControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    core: BluetoothControllerTimePendingCore<
        BluetoothPassiveScanControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Terminal scanner preparation with the exact task service retained.
#[must_use = "the task owner and scanner preparation outcome must be handled together"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPassiveScanControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: BluetoothPassiveScanControllerPreparationOutcome,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPassiveScanControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

/// Result of one bounded passive scanner controller-time observation.
#[must_use = "retain Pending or consume the terminal task and scanner result"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPassiveScanControllerPreparationStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
{
    Pending(BluetoothPassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>),
    Terminal(BluetoothPassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Lossless rejection before or during a first scanner preparation.
#[must_use = "retain the source-owned current or terminal scanner transaction"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothPassiveScanControllerInitialPreparationFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Rejected {
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPassiveScanControllerPreparationError,
    },
    Terminal(BluetoothPassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Finite reason a checked-out connection event returned to CPU ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionControllerPreparationError {
    ControllerTime(crate::BluetoothControllerTimeAcquisitionError),
    TimingWindow,
    Event(crate::scheduler::BluetoothPeripheralConnectionFirstEventPreparationError),
    EmptyList(crate::BluetoothSchedulerEmptyListMergeError),
}

/// Terminal result of one source-ordered first connection preparation.
#[must_use = "publish the connection item or retain every returned owner"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionControllerPreparationOutcome {
    /// The sole allocation was already checked out; no input was consumed.
    RuntimeUnavailable {
        error: crate::BluetoothPeripheralConnectionRuntimeBeginError,
        connection: open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
        packet_start: crate::BluetoothLe1MPacketStartTiming,
    },
    /// Preparation was rejected after consuming the packet-time input.
    Rejected {
        error: BluetoothPeripheralConnectionControllerPreparationError,
        connection: open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
    },
    /// The selected item is joined to the CPU-owned common scheduler list.
    Prepared(crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared),
}

#[cfg(target_arch = "riscv32")]
enum BluetoothPeripheralConnectionControllerPreparationPhase {
    Sequence(crate::scheduler::BluetoothPeripheralConnectionFirstPreSequence),
}

#[cfg(target_arch = "riscv32")]
struct BluetoothPeripheralConnectionControllerPreparationTimeOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<BluetoothPeripheralConnectionControllerPreparationPhase>,
    cancelled: Option<BluetoothPeripheralConnectionControllerPreparationOutcome>,
}

/// One exact in-flight sequence-deadline observation for a connection event.
#[must_use = "recheck or explicitly cancel the exact connection time request"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    core: BluetoothControllerTimePendingCore<
        BluetoothPeripheralConnectionControllerPreparationTimeOwner<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    >,
}

/// Terminal connection preparation with the exact task service retained.
#[must_use = "the task owner and connection outcome must be handled together"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: BluetoothPeripheralConnectionControllerPreparationOutcome,
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Recover the retained Controller and exact connection outcome.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPeripheralConnectionControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

/// Result of one bounded connection sequence-time observation.
#[must_use = "retain Pending or consume the terminal task and connection result"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionControllerPreparationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Pending(
        BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Terminal(
        BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ),
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
    TransmitterFirstAlwaysAwakeTiming {
        owner: crate::BluetoothDtmPreparedTxGraph,
        link_state: crate::BluetoothDtmLinkStateReset,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        requested_interval_micros: u16,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    ReceiverFirstAlwaysAwakeTiming {
        owner: crate::BluetoothDtmReceiverCpuOwned,
        link_state: crate::BluetoothDtmLinkStateReset,
        channel: crate::BluetoothDtmChannel,
        phy: crate::BluetoothDtmPhy,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    ReceiverRecurringAlwaysAwakeTiming {
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
        epoch: crate::BluetoothControllerSchedulerEpoch,
    },
    ReceiverRecurringCurrent {
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
        epoch: crate::BluetoothControllerSchedulerEpoch,
        timing_ready: crate::BluetoothAlwaysAwakeTimingReady,
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
    BluetoothLegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothLegacyAdvertisingControllerPreparationOutcome,
    ) -> BluetoothLegacyAdvertisingControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothLegacyAdvertisingControllerPreparationStep::Terminal(
            BluetoothLegacyAdvertisingControllerPreparationTerminal {
                controller: BluetoothControllerSchedulerEpochRetained { controller },
                outcome,
            },
        )
    }

    /// Perform one bounded observation of the exact advertising time request.
    pub fn recheck(
        self,
    ) -> BluetoothLegacyAdvertisingControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                return BluetoothLegacyAdvertisingControllerPreparationStep::Pending(Self { core });
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let phase = owner
                    .phase
                    .take()
                    .expect("failed advertising time recheck retains its exact phase");
                let outcome = owner
                    .controller
                    .cancel_legacy_advertising_preparation_phase(
                        phase,
                        controller_time_event_error(error),
                    );
                return Self::terminal(owner.controller, outcome);
            }
        };
        let phase = owner
            .phase
            .take()
            .expect("completed advertising time request retains its exact phase");
        let mut controller = owner.controller;
        match phase {
            BluetoothLegacyAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                reset,
                now,
            } => {
                let epoch = now.epoch();
                let current = crate::BluetoothSchedulerInstant::from_image(now.micros());
                let radio_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample)
                    .into_scheduler_instant();
                let timing = crate::BluetoothLegacyAdvertisingTimingObservation {
                    current,
                    radio_ready,
                    epoch,
                };
                let candidate = match reset
                    .form_first_event_candidate(timing, controller.runtime.scheduler_config())
                {
                    Ok(candidate) => candidate,
                    Err(failure) => {
                        controller
                            .restore_legacy_advertising_cancelled(failure.into_reset().cancel());
                        return Self::terminal(
                            controller,
                            BluetoothLegacyAdvertisingControllerPreparationOutcome::Rejected(
                                BluetoothLegacyAdvertisingControllerPreparationError::TimingWindow,
                            ),
                        );
                    }
                };
                match controller.begin_legacy_advertising_preparation_time(
                    BluetoothLegacyAdvertisingControllerPreparationPhase::Admission(candidate),
                ) {
                    Ok(pending) => {
                        BluetoothLegacyAdvertisingControllerPreparationStep::Pending(pending)
                    }
                    Err(terminal) => {
                        BluetoothLegacyAdvertisingControllerPreparationStep::Terminal(terminal)
                    }
                }
            }
            BluetoothLegacyAdvertisingControllerPreparationPhase::Admission(candidate) => {
                let admitted = match controller.runtime.admit_legacy_advertising_first_event(
                    candidate,
                    crate::BluetoothLegacyAdvertisingAdmissionObservation { sample },
                ) {
                    Ok(admitted) => admitted,
                    Err(failure) => {
                        let error = failure.error();
                        controller.restore_legacy_advertising_cancelled(
                            failure.into_candidate().cancel(),
                        );
                        return Self::terminal(
                            controller,
                            BluetoothLegacyAdvertisingControllerPreparationOutcome::Rejected(
                                BluetoothLegacyAdvertisingControllerPreparationError::Event(error),
                            ),
                        );
                    }
                };
                match controller.begin_legacy_advertising_preparation_time(
                    BluetoothLegacyAdvertisingControllerPreparationPhase::Sequence(admitted),
                ) {
                    Ok(pending) => {
                        BluetoothLegacyAdvertisingControllerPreparationStep::Pending(pending)
                    }
                    Err(terminal) => {
                        BluetoothLegacyAdvertisingControllerPreparationStep::Terminal(terminal)
                    }
                }
            }
            BluetoothLegacyAdvertisingControllerPreparationPhase::Sequence(admitted) => {
                let prepared = match controller.runtime.prepare_legacy_advertising_first_event(
                    admitted,
                    crate::BluetoothLegacyAdvertisingSequenceObservation { sample },
                ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        controller.restore_legacy_advertising_cancelled(
                            failure.into_candidate().cancel(),
                        );
                        return Self::terminal(
                            controller,
                            BluetoothLegacyAdvertisingControllerPreparationOutcome::Rejected(
                                BluetoothLegacyAdvertisingControllerPreparationError::Event(error),
                            ),
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_legacy_advertising_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::terminal(
                        controller,
                        BluetoothLegacyAdvertisingControllerPreparationOutcome::Prepared(merged),
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let cancelled = controller
                            .runtime
                            .cancel_legacy_advertising_first_event(failure.into_prepared());
                        controller.restore_legacy_advertising_cancelled(cancelled);
                        Self::terminal(
                            controller,
                            BluetoothLegacyAdvertisingControllerPreparationOutcome::Rejected(
                                BluetoothLegacyAdvertisingControllerPreparationError::EmptyList(
                                    error,
                                ),
                            ),
                        )
                    }
                }
            }
        }
    }

    /// Cancel the exact unpublished phase and restore the advertising runtime.
    pub fn cancel(
        self,
    ) -> BluetoothLegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
    {
        let mut owner = match self.core.cancel() {
            Ok(owner) => owner,
            Err(failure) => failure.into_parts().0,
        };
        let outcome = owner
            .cancelled
            .take()
            .expect("explicit advertising time cancellation records its restored outcome");
        BluetoothLegacyAdvertisingControllerPreparationTerminal {
            controller: BluetoothControllerSchedulerEpochRetained {
                controller: owner.controller,
            },
            outcome,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothLegacyAdvertisingControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
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
            Ok(()) => crate::BluetoothControllerTimeAcquisitionError::Cancelled,
            Err(error) => controller_time_event_error(error),
        };
        let phase = self
            .phase
            .take()
            .expect("private advertising time owner retains one exact preparation phase");
        self.cancelled = Some(
            self.controller
                .cancel_legacy_advertising_preparation_phase(phase, error),
        );
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
    BluetoothPassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothPassiveScanControllerPreparationOutcome,
    ) -> BluetoothPassiveScanControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothPassiveScanControllerPreparationStep::Terminal(
            BluetoothPassiveScanControllerPreparationTerminal {
                controller: BluetoothControllerSchedulerEpochRetained { controller },
                outcome,
            },
        )
    }

    /// Perform one bounded observation of the exact scanner time request.
    pub fn recheck(
        self,
    ) -> BluetoothPassiveScanControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                return BluetoothPassiveScanControllerPreparationStep::Pending(Self { core });
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let phase = owner
                    .phase
                    .take()
                    .expect("failed scanner time recheck retains its exact phase");
                let outcome = owner.controller.cancel_passive_scan_preparation_phase(
                    phase,
                    controller_time_event_error(error),
                );
                return Self::terminal(owner.controller, outcome);
            }
        };
        let phase = owner
            .phase
            .take()
            .expect("completed scanner time request retains its exact phase");
        let mut controller = owner.controller;
        match phase {
            BluetoothPassiveScanControllerPreparationPhase::AlwaysAwakeTiming {
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
                let timing =
                    crate::passive_scanning_timing::BluetoothPassiveScanTimingObservation {
                        current: crate::BluetoothSchedulerInstant::from_image(now.micros()),
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
                        controller.restore_passive_scan_graph(failure.into_graph());
                        return Self::terminal(
                            controller,
                            BluetoothPassiveScanControllerPreparationOutcome::Rejected(
                                BluetoothPassiveScanControllerPreparationError::TimingWindow,
                            ),
                        );
                    }
                };
                match controller.begin_passive_scan_preparation_time(
                    BluetoothPassiveScanControllerPreparationPhase::Admission { candidate, phase },
                ) {
                    Ok(pending) => BluetoothPassiveScanControllerPreparationStep::Pending(pending),
                    Err(terminal) => {
                        BluetoothPassiveScanControllerPreparationStep::Terminal(terminal)
                    }
                }
            }
            BluetoothPassiveScanControllerPreparationPhase::Admission { candidate, phase } => {
                let admitted = match controller.runtime.admit_passive_scan_first_event(
                    candidate,
                    crate::BluetoothPassiveScanAdmissionObservation { sample },
                ) {
                    Ok(admitted) => admitted,
                    Err(failure) => {
                        let error = failure.error();
                        controller.restore_passive_scan_graph(failure.into_candidate().cancel());
                        return Self::terminal(
                            controller,
                            BluetoothPassiveScanControllerPreparationOutcome::Rejected(
                                BluetoothPassiveScanControllerPreparationError::Event(error),
                            ),
                        );
                    }
                };
                match controller.begin_passive_scan_preparation_time(
                    BluetoothPassiveScanControllerPreparationPhase::Sequence { admitted, phase },
                ) {
                    Ok(pending) => BluetoothPassiveScanControllerPreparationStep::Pending(pending),
                    Err(terminal) => {
                        BluetoothPassiveScanControllerPreparationStep::Terminal(terminal)
                    }
                }
            }
            BluetoothPassiveScanControllerPreparationPhase::Sequence { admitted, phase } => {
                let prepared = match controller.runtime.prepare_passive_scan_first_event(
                    admitted,
                    crate::BluetoothPassiveScanSequenceObservation { sample },
                ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        controller.restore_passive_scan_graph(failure.into_candidate().cancel());
                        return Self::terminal(
                            controller,
                            BluetoothPassiveScanControllerPreparationOutcome::Rejected(
                                BluetoothPassiveScanControllerPreparationError::Event(error),
                            ),
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_passive_scan_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::terminal(
                        controller,
                        BluetoothPassiveScanControllerPreparationOutcome::Prepared {
                            merged,
                            phase,
                        },
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let graph = controller
                            .runtime
                            .cancel_passive_scan_first_event(failure.into_prepared());
                        controller.restore_passive_scan_graph(graph);
                        Self::terminal(
                            controller,
                            BluetoothPassiveScanControllerPreparationOutcome::Rejected(
                                BluetoothPassiveScanControllerPreparationError::EmptyList(error),
                            ),
                        )
                    }
                }
            }
        }
    }

    /// Cancel the unpublished phase and return the graph to its sole runtime.
    pub fn cancel(
        self,
    ) -> BluetoothPassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY> {
        let mut owner = match self.core.cancel() {
            Ok(owner) => owner,
            Err(failure) => failure.into_parts().0,
        };
        let outcome = owner
            .cancelled
            .take()
            .expect("explicit scanner time cancellation records its restored outcome");
        BluetoothPassiveScanControllerPreparationTerminal {
            controller: BluetoothControllerSchedulerEpochRetained {
                controller: owner.controller,
            },
            outcome,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothPassiveScanControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
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
            Ok(()) => crate::BluetoothControllerTimeAcquisitionError::Cancelled,
            Err(error) => controller_time_event_error(error),
        };
        let phase = self
            .phase
            .take()
            .expect("private scanner time owner retains one exact preparation phase");
        self.cancelled = Some(
            self.controller
                .cancel_passive_scan_preparation_phase(phase, error),
        );
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
    BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn terminal(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothPeripheralConnectionControllerPreparationOutcome,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
            BluetoothPeripheralConnectionControllerPreparationTerminal {
                controller: BluetoothControllerSchedulerEpochRetained { controller },
                outcome,
            },
        )
    }

    /// Perform one bounded observation of the connection sequence deadline.
    pub fn recheck(
        self,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                return BluetoothPeripheralConnectionControllerPreparationStep::Pending(Self {
                    core,
                });
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let phase = owner
                    .phase
                    .take()
                    .expect("failed connection time recheck retains its exact phase");
                let outcome = owner
                    .controller
                    .cancel_peripheral_connection_preparation_phase(
                        phase,
                        controller_time_event_error(error),
                    );
                return Self::terminal(owner.controller, outcome);
            }
        };
        let phase = owner
            .phase
            .take()
            .expect("completed connection time request retains its exact phase");
        let mut controller = owner.controller;
        match phase {
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence(admitted) => {
                let default_tx_power = controller
                    .peripheral_connection_resources
                    .default_tx_power_dbm();
                let direction_finding_workspace = controller.direction_finding_workspace;
                let prepared = match controller
                    .runtime
                    .prepare_peripheral_connection_first_event(
                        admitted,
                        crate::scheduler::BluetoothPeripheralConnectionSequenceObservation {
                            sample,
                        },
                        default_tx_power,
                        direction_finding_workspace,
                    ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = failure.into_candidate().cancel();
                        controller.restore_peripheral_connection_allocation(allocation);
                        return Self::terminal(
                            controller,
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error:
                                    BluetoothPeripheralConnectionControllerPreparationError::Event(
                                        error,
                                    ),
                                connection,
                            },
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_peripheral_connection_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::terminal(
                        controller,
                        BluetoothPeripheralConnectionControllerPreparationOutcome::Prepared(merged),
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = controller
                            .runtime
                            .cancel_peripheral_connection_first_event(failure.into_prepared());
                        controller.restore_peripheral_connection_allocation(allocation);
                        Self::terminal(
                            controller,
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error: BluetoothPeripheralConnectionControllerPreparationError::EmptyList(
                                    error,
                                ),
                                connection,
                            },
                        )
                    }
                }
            }
        }
    }

    /// Cancel the unpublished sequence request and restore the connection allocation.
    pub fn cancel(
        self,
    ) -> BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
    {
        let mut owner = match self.core.cancel() {
            Ok(owner) => owner,
            Err(failure) => failure.into_parts().0,
        };
        let outcome = owner
            .cancelled
            .take()
            .expect("explicit connection time cancellation records its restored outcome");
        BluetoothPeripheralConnectionControllerPreparationTerminal {
            controller: BluetoothControllerSchedulerEpochRetained {
                controller: owner.controller,
            },
            outcome,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothPeripheralConnectionControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
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
            Ok(()) => crate::BluetoothControllerTimeAcquisitionError::Cancelled,
            Err(error) => controller_time_event_error(error),
        };
        let phase = self
            .phase
            .take()
            .expect("private connection time owner retains one exact preparation phase");
        self.cancelled = Some(
            self.controller
                .cancel_peripheral_connection_preparation_phase(phase, error),
        );
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
            BluetoothDtmControllerPreparationPhase::TransmitterFirstAlwaysAwakeTiming {
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
            BluetoothDtmControllerPreparationPhase::ReceiverFirstAlwaysAwakeTiming {
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
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringAlwaysAwakeTiming {
                owner,
                epoch,
            } => {
                let timing_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample);
                match controller.begin_dtm_preparation_time(
                    BluetoothDtmControllerPreparationPhase::ReceiverRecurringCurrent {
                        owner,
                        epoch,
                        timing_ready,
                    },
                ) {
                    Ok(pending) => BluetoothDtmControllerPreparationStep::Pending(pending),
                    Err(terminal) => BluetoothDtmControllerPreparationStep::Terminal(terminal),
                }
            }
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringCurrent {
                owner,
                epoch,
                timing_ready,
            } => {
                let epoch = epoch.reanchor(&sample);
                *controller.scheduler_epoch = Some(epoch);
                let now =
                    crate::controller_time::BluetoothControllerSchedulerNow::from_retained_epoch(
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
            Ok(()) => crate::BluetoothControllerTimeAcquisitionError::Cancelled,
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
    fn restore_legacy_advertising_cancelled(
        &mut self,
        cancelled: crate::BluetoothLegacyAdvertisingCancelled<'static>,
    ) {
        if self
            .legacy_advertising_resources
            .restore_cancelled(cancelled)
            .is_err()
        {
            panic!("an advertising phase returned owners to a different runtime");
        }
    }

    fn cancel_legacy_advertising_preparation_phase(
        &mut self,
        phase: BluetoothLegacyAdvertisingControllerPreparationPhase,
        error: crate::BluetoothControllerTimeAcquisitionError,
    ) -> BluetoothLegacyAdvertisingControllerPreparationOutcome {
        let cancelled = match phase {
            BluetoothLegacyAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                reset,
                ..
            } => reset.cancel(),
            BluetoothLegacyAdvertisingControllerPreparationPhase::Admission(candidate) => {
                candidate.cancel()
            }
            BluetoothLegacyAdvertisingControllerPreparationPhase::Sequence(admitted) => self
                .runtime
                .cancel_legacy_advertising_first_pre_sequence(admitted),
        };
        self.restore_legacy_advertising_cancelled(cancelled);
        BluetoothLegacyAdvertisingControllerPreparationOutcome::Rejected(
            BluetoothLegacyAdvertisingControllerPreparationError::ControllerTime(error),
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc begin rejection retains the restored task and typed outcome"
    )]
    fn begin_legacy_advertising_preparation_time(
        mut self,
        phase: BluetoothLegacyAdvertisingControllerPreparationPhase,
    ) -> Result<
        BluetoothLegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothLegacyAdvertisingControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome = self.cancel_legacy_advertising_preparation_phase(
                    phase,
                    controller_time_begin_error(error),
                );
                return Err(BluetoothLegacyAdvertisingControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(BluetoothLegacyAdvertisingControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothLegacyAdvertisingControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                    cancelled: None,
                },
                request,
            ),
        })
    }

    fn restore_passive_scan_graph(
        &mut self,
        graph: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCpuOwned,
    ) {
        if self.passive_scan_resources.restore_idle(graph).is_err() {
            panic!("a scanner phase returned a graph to a different runtime");
        }
    }

    fn cancel_passive_scan_preparation_phase(
        &mut self,
        phase: BluetoothPassiveScanControllerPreparationPhase,
        error: crate::BluetoothControllerTimeAcquisitionError,
    ) -> BluetoothPassiveScanControllerPreparationOutcome {
        let graph = match phase {
            BluetoothPassiveScanControllerPreparationPhase::AlwaysAwakeTiming { graph, .. } => {
                graph
            }
            BluetoothPassiveScanControllerPreparationPhase::Admission { candidate, .. } => {
                candidate.cancel()
            }
            BluetoothPassiveScanControllerPreparationPhase::Sequence { admitted, .. } => self
                .runtime
                .cancel_passive_scan_first_pre_sequence(admitted),
        };
        self.restore_passive_scan_graph(graph);
        BluetoothPassiveScanControllerPreparationOutcome::Rejected(
            BluetoothPassiveScanControllerPreparationError::ControllerTime(error),
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the Controller and restored scanner graph"
    )]
    fn begin_passive_scan_preparation_time(
        mut self,
        phase: BluetoothPassiveScanControllerPreparationPhase,
    ) -> Result<
        BluetoothPassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPassiveScanControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome = self.cancel_passive_scan_preparation_phase(
                    phase,
                    controller_time_begin_error(error),
                );
                return Err(BluetoothPassiveScanControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(BluetoothPassiveScanControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothPassiveScanControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                    cancelled: None,
                },
                request,
            ),
        })
    }

    fn restore_peripheral_connection_allocation(
        &mut self,
        allocation: crate::BluetoothPeripheralConnectionRuntimeAllocation,
    ) {
        if self
            .peripheral_connection_resources
            .restore_idle(allocation)
            .is_err()
        {
            panic!("a connection phase returned an allocation to a different runtime");
        }
    }

    fn cancel_peripheral_connection_preparation_phase(
        &mut self,
        phase: BluetoothPeripheralConnectionControllerPreparationPhase,
        error: crate::BluetoothControllerTimeAcquisitionError,
    ) -> BluetoothPeripheralConnectionControllerPreparationOutcome {
        let (allocation, connection) = match phase {
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence(admitted) => self
                .runtime
                .cancel_peripheral_connection_first_pre_sequence(admitted),
        };
        self.restore_peripheral_connection_allocation(allocation);
        BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
            error: BluetoothPeripheralConnectionControllerPreparationError::ControllerTime(error),
            connection,
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the Controller and restored connection allocation"
    )]
    fn begin_peripheral_connection_preparation_time(
        mut self,
        phase: BluetoothPeripheralConnectionControllerPreparationPhase,
    ) -> Result<
        BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome = self.cancel_peripheral_connection_preparation_phase(
                    phase,
                    controller_time_begin_error(error),
                );
                return Err(BluetoothPeripheralConnectionControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(BluetoothPeripheralConnectionControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothPeripheralConnectionControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                    cancelled: None,
                },
                request,
            ),
        })
    }

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

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection returns the complete affine advertising event"
    )]
    pub(crate) fn restore_legacy_advertising_completed_disabled(
        &mut self,
        completed: crate::BluetoothLegacyAdvertisingEventCompleted<'static>,
    ) -> Result<(), crate::BluetoothLegacyAdvertisingEventCompleted<'static>> {
        self.legacy_advertising_resources
            .restore_completed_disabled(completed)
    }

    fn new_dtm_link_state_reset(
        &self,
        role: crate::BluetoothDtmRole,
    ) -> crate::BluetoothDtmLinkStateReset {
        crate::BluetoothDtmLinkStateReset::new(self.dtm_resources.default_tx_power_dbm(), role)
    }

    fn cancel_dtm_preparation_phase(
        &mut self,
        phase: BluetoothDtmControllerPreparationPhase,
        error: crate::BluetoothControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerPreparationOutcome {
        match phase {
            BluetoothDtmControllerPreparationPhase::TransmitterFirstAlwaysAwakeTiming {
                owner,
                ..
            } => BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(self
                .runtime
                .reject_dtm_transmitter_first_before_stage(owner, error))),
            BluetoothDtmControllerPreparationPhase::ReceiverFirstAlwaysAwakeTiming {
                owner,
                ..
            } => BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(self
                .runtime
                .reject_dtm_receiver_first_before_stage(owner, error))),
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringAlwaysAwakeTiming {
                owner,
                ..
            }
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
    fn begin_dtm_always_awake_timing(
        mut self,
        phase: BluetoothDtmControllerPreparationPhase,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let outcome =
                    self.cancel_dtm_preparation_phase(phase, controller_time_begin_error(error));
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
                let outcome =
                    self.cancel_dtm_preparation_phase(phase, controller_time_begin_error(error));
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
        owner: crate::BluetoothDtmActiveReceiverCpuOwned,
    ) -> Result<
        BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let controller = self.controller;
        let epoch = (*controller.scheduler_epoch)
            .expect("the retained scheduler epoch cannot lose its stored epoch");
        controller.begin_dtm_always_awake_timing(
            BluetoothDtmControllerPreparationPhase::ReceiverRecurringAlwaysAwakeTiming {
                owner,
                epoch,
            },
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

    /// Apply one fresh sequence observation to an already reserved successor.
    pub(crate) fn finish_legacy_advertising_recurring_event(
        self,
        admitted: crate::BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    ) -> BluetoothLegacyAdvertisingRecurringSequenceCompletion<'runtime, S, SCHEDULER_CAPACITY>
    {
        let Self {
            mut controller,
            sample,
            ..
        } = self;
        let prepared = match controller
            .runtime
            .prepare_legacy_advertising_recurring_event(
                admitted,
                crate::BluetoothLegacyAdvertisingSequenceObservation { sample },
            ) {
            Ok(prepared) => prepared,
            Err(failure) => {
                return BluetoothLegacyAdvertisingRecurringSequenceCompletion::EventRejected {
                    task: controller,
                    failure,
                };
            }
        };
        match controller
            .runtime
            .prepare_legacy_advertising_empty_list_merge(prepared)
        {
            Ok(merged) => BluetoothLegacyAdvertisingRecurringSequenceCompletion::Prepared {
                task: controller,
                merged,
            },
            Err(failure) => {
                BluetoothLegacyAdvertisingRecurringSequenceCompletion::EmptyListRejected {
                    task: controller,
                    failure,
                }
            }
        }
    }

    /// Begin the source-ordered first legacy-advertising transaction.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete current or preparation owner"
    )]
    pub(crate) fn begin_legacy_advertising_first_event(
        self,
        set: open_esp_radio_bluetooth_ll::advertising::LegacyNonconnectableAdvertisingSet<'static>,
    ) -> Result<
        BluetoothLegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothLegacyAdvertisingControllerInitialPreparationFailure<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
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
                    BluetoothLegacyAdvertisingControllerInitialPreparationFailure::Rejected {
                        current,
                        error: BluetoothLegacyAdvertisingControllerPreparationError::Runtime(error),
                    },
                );
            }
        };
        let (prepared, default_tx_power) = event.into_parts();
        let reset = match prepared.reset_link_state(default_tx_power) {
            Ok(reset) => reset,
            Err(failure) => {
                let error = failure.error();
                current
                    .controller
                    .restore_legacy_advertising_cancelled(failure.into_prepared().cancel());
                return Err(
                    BluetoothLegacyAdvertisingControllerInitialPreparationFailure::Rejected {
                        current,
                        error: BluetoothLegacyAdvertisingControllerPreparationError::LinkState(
                            error,
                        ),
                    },
                );
            }
        };
        let (controller, now) = current.into_parts();
        controller
            .begin_legacy_advertising_preparation_time(
                BluetoothLegacyAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                    reset,
                    now,
                },
            )
            .map_err(BluetoothLegacyAdvertisingControllerInitialPreparationFailure::Terminal)
    }

    /// Begin the source-ordered first passive scanner transaction.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete current or scanner owner"
    )]
    pub(crate) fn begin_passive_scan_first_event(
        self,
        parameters: open_esp_radio_bluetooth_ll::scanning::LegacyPassiveScanParameters,
        channel: open_esp_radio_bluetooth_ll::scanning::PrimaryScanChannel,
        previous_phase: Option<crate::BluetoothPassiveScanEventPhase>,
    ) -> Result<
        BluetoothPassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPassiveScanControllerInitialPreparationFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let current = self;
        let graph = match current.controller.passive_scan_resources.begin_event() {
            Ok(graph) => graph,
            Err(error) => {
                return Err(
                    BluetoothPassiveScanControllerInitialPreparationFailure::Rejected {
                        current,
                        error: BluetoothPassiveScanControllerPreparationError::Runtime(error),
                    },
                );
            }
        };
        let (controller, now) = current.into_parts();
        controller
            .begin_passive_scan_preparation_time(
                BluetoothPassiveScanControllerPreparationPhase::AlwaysAwakeTiming {
                    graph,
                    channel: crate::passive_scanning::lower_primary_channel(channel),
                    parameters,
                    previous_phase,
                    now,
                },
            )
            .map_err(BluetoothPassiveScanControllerInitialPreparationFailure::Terminal)
    }

    /// Begin the first peripheral connection event from its causal packet time.
    ///
    /// The current sample authorizes timeline admission. A distinct later
    /// request authorizes descriptor sequencing after overlap resolution. Every
    /// rejection restores the exact connection allocation before returning the
    /// retained Controller epoch.
    pub fn begin_peripheral_connection_first_event(
        self,
        connection: open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
        packet_start: crate::BluetoothLe1MPacketStartTiming,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        let Self {
            mut controller,
            epoch,
            sample,
        } = self;
        let allocation = match controller.peripheral_connection_resources.begin_event() {
            Ok(allocation) => allocation,
            Err(error) => {
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        outcome:
                            BluetoothPeripheralConnectionControllerPreparationOutcome::RuntimeUnavailable {
                                error,
                                connection,
                                packet_start,
                            },
                    },
                );
            }
        };
        let prepared = allocation.prepare_first_event(connection, packet_start);
        let candidate = match prepared
            .project_scheduler_window(epoch, controller.runtime.scheduler_config())
        {
            Ok(candidate) => candidate,
            Err(prepared) => {
                let (allocation, connection) = prepared.cancel();
                controller.restore_peripheral_connection_allocation(allocation);
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        outcome:
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error: BluetoothPeripheralConnectionControllerPreparationError::TimingWindow,
                                connection,
                            },
                    },
                );
            }
        };
        let admitted = match controller.runtime.admit_peripheral_connection_first_event(
            candidate,
            crate::scheduler::BluetoothPeripheralConnectionAdmissionObservation { sample },
        ) {
            Ok(admitted) => admitted,
            Err(failure) => {
                let error = failure.error();
                let (allocation, connection) = failure.into_candidate().cancel();
                controller.restore_peripheral_connection_allocation(allocation);
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        outcome:
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error:
                                    BluetoothPeripheralConnectionControllerPreparationError::Event(
                                        error,
                                    ),
                                connection,
                            },
                    },
                );
            }
        };
        match controller.begin_peripheral_connection_preparation_time(
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence(admitted),
        ) {
            Ok(pending) => BluetoothPeripheralConnectionControllerPreparationStep::Pending(pending),
            Err(terminal) => {
                BluetoothPeripheralConnectionControllerPreparationStep::Terminal(terminal)
            }
        }
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
            .begin_dtm_always_awake_timing(
                BluetoothDtmControllerPreparationPhase::TransmitterFirstAlwaysAwakeTiming {
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
    /// post-enable timing request. Admission and sequence then remain private affine
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
            .begin_dtm_always_awake_timing(
                BluetoothDtmControllerPreparationPhase::ReceiverFirstAlwaysAwakeTiming {
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
    /// This path has no post-enable timing phase in the reviewed vendor flow.
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

/// Failed stable publication retaining the Controller, storage and role runtimes.
#[must_use = "failed ISR publication returns every affine owner for inspection or retry"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerInterruptOwnerPublicationFailure<
    P,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothInterruptOwnerStorage,
{
    controller:
        BluetoothControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    storage: S,
    dtm_resources: crate::BluetoothDtmRuntimeResources,
    legacy_advertising_resources: crate::BluetoothLegacyAdvertisingRuntimeResources,
    passive_scan_resources: crate::BluetoothPassiveScanRuntimeResources,
    peripheral_connection_resources: crate::BluetoothPeripheralConnectionRuntimeResources,
    error: S::Error,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerInterruptOwnersPublished<P, S, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
where
    S: BluetoothModemLpTimerSoftwareOwnerStorage,
{
    /// Borrow the published hardware graph as disjoint interrupt and task endpoints.
    ///
    /// The caller must retain this owner in stable storage for the complete
    /// routed lifetime. HCI remains a separate protocol resource until the
    /// post-publication binding transition.
    pub(crate) fn split_hardware_runtime<'runtime>(
        &'runtime mut self,
    ) -> BluetoothControllerPublishedHardwareRuntimeEndpoints<
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
            ..
        } = self;
        let direction_finding_workspace = initialized.direction_finding_workspace_link();
        let (
            BluetoothControllerRuntimeEndpoints {
                interrupt,
                task,
                modem_timer,
            },
            ble_phy_timing,
        ) = initialized.split_runtime();
        let interrupt = BluetoothControllerPublishedInterruptService {
            storage: _storage,
            runtime: interrupt,
            mailbox: post_unlink_mailbox,
        };
        let task = BluetoothControllerPublishedTaskService {
            storage: _storage,
            runtime: task,
            mailbox: post_unlink_mailbox,
            dtm_resources,
            legacy_advertising_resources,
            passive_scan_resources,
            peripheral_connection_resources,
            direction_finding_workspace,
            ble_phy_timing,
            scheduler_epoch,
        };
        let modem_timer = BluetoothControllerModemTimerTask::new(_storage, modem_timer);
        BluetoothControllerPublishedHardwareRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
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
}

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Join this powered epoch's global DF workspace to one connection image.
    #[allow(
        dead_code,
        reason = "the next peripheral scheduler-publication transition consumes this state"
    )]
    pub(crate) fn install_peripheral_connection_direction_finding_workspace(
        &self,
        prepared: crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventFieldsPrepared,
    ) -> crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared
    {
        prepared.install_direction_finding_workspace(self.direction_finding_workspace)
    }

    /// Normalize the hardware timestamp captured beside one received LE 1M PDU.
    ///
    /// This uses the persistent scheduler epoch and the calibration retained by
    /// the initialized BLE PHY storage. It performs no MMIO, does not sample
    /// `now()`, and does not advance the scheduler epoch.
    pub fn normalize_le_1m_packet_start(
        &mut self,
        packet: &open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedPdu,
    ) -> Result<crate::BluetoothLe1MPacketStartTiming, BluetoothLePacketStartTimingError> {
        let epoch = (*self.scheduler_epoch)
            .ok_or(BluetoothLePacketStartTimingError::SchedulerEpochUnavailable)?;
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

    /// Publish the first advertising item through the same exclusive head edge.
    #[expect(
        clippy::result_large_err,
        reason = "pre-MMIO rejection returns the complete advertising graph"
    )]
    pub fn publish_legacy_advertising_scheduler_head<'a>(
        &mut self,
        merged: crate::BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingSchedulerHeadPublished<'a>,
        crate::BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure<'a>,
    > {
        self.runtime
            .publish_legacy_advertising_scheduler_head(merged)
    }

    /// Publish the first passive scanner item through the exclusive head edge.
    pub(crate) fn publish_passive_scan_scheduler_head(
        &mut self,
        merged: crate::BluetoothPassiveScanEmptySchedulerMergePrepared,
    ) -> Result<
        crate::BluetoothPassiveScanSchedulerHeadPublished,
        crate::BluetoothPassiveScanSchedulerHeadPublicationFailure,
    > {
        self.runtime.publish_passive_scan_scheduler_head(merged)
    }

    /// Publish selector-two RX memory and the first connection scheduler head.
    #[allow(
        clippy::result_large_err,
        reason = "pre-MMIO rejection returns the complete no-alloc connection graph"
    )]
    pub fn publish_peripheral_connection_scheduler_head(
        &mut self,
        merged: crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        crate::BluetoothPeripheralConnectionSchedulerHeadPublished,
        crate::BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        self.runtime
            .publish_peripheral_connection_scheduler_head(merged)
    }

    /// Cancel an unpublished connection merge and restore its sole allocation.
    ///
    /// A scheduler-identity mismatch returns the unchanged merge, because this
    /// Controller cannot safely restore an item owned by another list epoch.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity rejection retains the complete connection merge"
    )]
    pub fn cancel_peripheral_connection_scheduler_merge(
        &mut self,
        merged: crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
        crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    > {
        let prepared = self
            .runtime
            .cancel_peripheral_connection_empty_list_merge(merged)?;
        let (allocation, connection) = self
            .runtime
            .cancel_peripheral_connection_first_event(prepared);
        self.restore_peripheral_connection_allocation(allocation);
        Ok(connection)
    }
}

#[cfg(target_arch = "riscv32")]
impl<S> BluetoothControllerPublishedInterruptService<'_, S> {
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
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerInterruptOwnerPublicationFailure<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
where
    S: BluetoothInterruptOwnerStorage,
{
    /// Inspect the exact platform rejection.
    pub const fn error(&self) -> &S::Error {
        &self.error
    }

    /// Recover the complete pre-publication Controller, storage and role runtimes.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        S,
        crate::BluetoothDtmRuntimeResources,
        crate::BluetoothLegacyAdvertisingRuntimeResources,
        crate::BluetoothPassiveScanRuntimeResources,
        crate::BluetoothPeripheralConnectionRuntimeResources,
        S::Error,
    ) {
        (
            self.controller,
            self.storage,
            self.dtm_resources,
            self.legacy_advertising_resources,
            self.passive_scan_resources,
            self.peripheral_connection_resources,
            self.error,
        )
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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
    /// the storage capability and unmodified role runtimes. Success still leaves
    /// every CPU route inactive.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return every affine powered owner"
    )]
    pub fn publish_interrupt_owners<S>(
        self,
        storage: S,
        dtm_resources: crate::BluetoothDtmRuntimeResources,
        legacy_advertising_resources: crate::BluetoothLegacyAdvertisingRuntimeResources,
        passive_scan_resources: crate::BluetoothPassiveScanRuntimeResources,
        peripheral_connection_resources: crate::BluetoothPeripheralConnectionRuntimeResources,
    ) -> Result<
        BluetoothControllerInterruptOwnersPublished<
            P,
            S::Published,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
        BluetoothControllerInterruptOwnerPublicationFailure<
            P,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
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
                legacy_advertising_resources,
                passive_scan_resources,
                peripheral_connection_resources,
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
                    legacy_advertising_resources,
                    passive_scan_resources,
                    peripheral_connection_resources,
                    error,
                })
            }
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Transfer both disjoint register owners into their pre-route states.
    ///
    /// This is an ownership-only transition. It performs no MMIO and does not
    /// claim stable placement or a live interrupt epoch.
    pub fn stage_interrupt_owners(
        self,
    ) -> BluetoothControllerInterruptOwnersReady<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
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
impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerBlePhyEngineInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Prepare Controller IRQ output and then start the runtime timer once.
    ///
    /// The consuming BLE-PHY state proves that controller HAL, scheduler,
    /// low-power hardware, common PHY, BTBB and BLE PHY initialization
    /// all belong to this epoch. CPU routes remain inaccessible, so the lower
    /// unsafe interrupt prerequisite is discharged here and never exported.
    #[allow(
        unsafe_code,
        reason = "the complete Controller typestate proves the HAL interrupt prerequisites"
    )]
    pub fn prepare_controller_output_and_start_runtime_timer(
        mut self,
    ) -> BluetoothControllerOutputTimerStarted<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
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
