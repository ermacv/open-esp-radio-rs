//! Typed ESP-HAL routing primitives for all three Bluetooth interrupts.
//!
//! Stable publication is deliberately separate from a live interrupt epoch.
//! Both unique HAL owners are installed atomically before any CPU route can be
//! enabled. A borrowing bind transition joins that stable publication to one
//! full-controller dispatcher. Three adapter-owned handlers supply their
//! fixed semantic roles to that dispatcher, so integration cannot exchange
//! primary, modem-timer and NRT callbacks. The resulting affine epoch is the
//! only value that may keep those routes live.

#![forbid(unsafe_code)]

use core::cell::RefCell;

use critical_section::Mutex;
use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    peripherals::Interrupt,
    system::Cpu,
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothInterruptOwnerStorage, BluetoothModemLpTimerInterruptDispatchStorage,
    BluetoothModemLpTimerSoftwareOwnerStorage, BluetoothModemLpTimerStableInterruptStep,
    BluetoothNrtDefaultInterruptEpoch, BluetoothPrimaryInterruptStep,
    BluetoothSchedulerRunInterruptStorage, BluetoothSharedInterruptDispatchStorage,
    step_nrt_default_interrupt, step_primary_interrupt,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHal, BluetoothInterruptRegistersOwner,
    BluetoothModemLpTimerHandlerRegisterStep, BluetoothModemLpTimerInterruptReadyOwner,
    BluetoothModemLpTimerInterruptStep, BluetoothModemLpTimerSoftwarePendingOwner,
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerRunInterruptsPrepared,
    BluetoothSchedulerSoftwareListRemovalJoin,
};

use crate::bluetooth_route_policy::{
    BluetoothInterruptRouteState, BluetoothModemLpTimerInterruptAdmission,
    BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
    EspHalBluetoothInterruptRouteError, EspHalBluetoothInterruptStorageError,
    classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
    ready_owner_restore_is_admitted, service_stable_owner, validate_interrupt_storage,
};

pub(crate) const PRIMARY_INTERRUPT: Interrupt = Interrupt::BT_MAC;
pub(crate) const MODEM_LP_TIMER_INTERRUPT: Interrupt = Interrupt::MODEM_LP_TIMER;
pub(crate) const NRT_INTERRUPT: Interrupt = Interrupt::BT_MAC_INT1;
const ROUTE_PRIORITY: Priority = Priority::Priority3;

static INTERRUPT_REGISTERS: Mutex<RefCell<Option<BluetoothInterruptRegistersOwner>>> =
    Mutex::new(RefCell::new(None));
static MODEM_LP_TIMER: Mutex<RefCell<Option<StoredBluetoothModemLpTimerOwner>>> =
    Mutex::new(RefCell::new(None));
static BOUND_ROUTE_DISPATCH: Mutex<RefCell<Option<BoundRouteDispatch>>> =
    Mutex::new(RefCell::new(None));

/// Exact semantic role of the adapter-owned handler that entered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptSource {
    /// Controller primary source 124 (`BT_MAC`).
    Primary,
    /// Modem low-power timer source 127 (`MODEM_LP_TIMER`).
    ModemLpTimer,
    /// Controller default NRT source 133 (`BT_MAC_INT1`).
    NrtDefault,
}

impl EspHalBluetoothInterruptSource {
    const fn interrupt(self) -> Interrupt {
        match self {
            Self::Primary => PRIMARY_INTERRUPT,
            Self::ModemLpTimer => MODEM_LP_TIMER_INTERRUPT,
            Self::NrtDefault => NRT_INTERRUPT,
        }
    }
}

/// Required route disposition after one full Controller interrupt service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "fatal service failure must quarantine its asserted CPU route"]
pub enum EspHalBluetoothInterruptDisposition {
    /// Full service completed and the route remains live.
    Serviced,
    /// A fatal storage invariant failed; disable this asserted route in place.
    Quarantine,
}

#[derive(Clone, Copy)]
struct BoundRouteDispatch {
    core: Cpu,
    dispatch: fn(EspHalBluetoothInterruptSource) -> EspHalBluetoothInterruptDisposition,
    live: bool,
}

fn dispatch_bound_source(source: EspHalBluetoothInterruptSource) {
    let bound = critical_section::with(|critical_section| {
        BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .filter(|route| route.live)
            .map(|route| (route.core, route.dispatch))
    });
    let Some((core, dispatch)) = bound else {
        return;
    };
    if dispatch(source) == EspHalBluetoothInterruptDisposition::Quarantine {
        critical_section::with(|critical_section| {
            let route = BOUND_ROUTE_DISPATCH.borrow_ref(critical_section);
            if route
                .as_ref()
                .is_some_and(|route| route.live && route.core == core)
            {
                interrupt::disable(core, source.interrupt());
            }
        });
    }
}

extern "C" fn bluetooth_primary_interrupt_handler() {
    dispatch_bound_source(EspHalBluetoothInterruptSource::Primary);
}

extern "C" fn bluetooth_modem_lp_timer_interrupt_handler() {
    dispatch_bound_source(EspHalBluetoothInterruptSource::ModemLpTimer);
}

extern "C" fn bluetooth_nrt_default_interrupt_handler() {
    dispatch_bound_source(EspHalBluetoothInterruptSource::NrtDefault);
}

const PRIMARY_HANDLER: InterruptHandler =
    InterruptHandler::new(bluetooth_primary_interrupt_handler, ROUTE_PRIORITY);
const MODEM_LP_TIMER_HANDLER: InterruptHandler =
    InterruptHandler::new(bluetooth_modem_lp_timer_interrupt_handler, ROUTE_PRIORITY);
const NRT_HANDLER: InterruptHandler =
    InterruptHandler::new(bluetooth_nrt_default_interrupt_handler, ROUTE_PRIORITY);

enum StoredBluetoothModemLpTimerOwner {
    Ready(BluetoothModemLpTimerInterruptReadyOwner),
    SoftwarePending(BluetoothModemLpTimerSoftwarePendingOwner),
}

impl StoredBluetoothModemLpTimerOwner {
    const fn phase(&self) -> BluetoothModemLpTimerStoragePhase {
        match self {
            Self::Ready(_) => BluetoothModemLpTimerStoragePhase::Ready,
            Self::SoftwarePending(_) => BluetoothModemLpTimerStoragePhase::SoftwarePending,
        }
    }
}

/// Stable process-wide slots for both Bluetooth ISR register owners.
///
/// Constructing this value performs no claim. Publication rejects any second
/// value while either process-wide slot is occupied.
#[derive(Clone, Copy, Debug, Default)]
pub struct EspHalBluetoothInterruptStorage;

impl EspHalBluetoothInterruptStorage {
    /// Construct an unclaimed reference to the process-wide storage boundary.
    pub const fn new() -> Self {
        Self
    }
}

/// Affine proof that both Bluetooth owners remain in stable ISR storage.
///
/// Dropping the lease is fail-stop: the owners remain published because no
/// verified live-route shutdown and Controller teardown path exists yet.
#[must_use = "published Bluetooth ISR owners remain process-wide until verified teardown"]
pub struct PublishedEspHalBluetoothInterruptOwners {
    _private: (),
}

/// Result of one finite source-127 register-only hard-handler entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothModemLpTimerInterruptStep {
    /// No timer owner is currently available in the stable slot.
    Unavailable,
    /// Software work already owns the timer; no register was touched.
    AwaitingSoftware,
    /// The first status observation was empty and restored the ready owner.
    Spurious,
    /// The common register phase completed without software work.
    Rearmed,
    /// The acknowledged register phase transferred ownership to task work.
    SoftwarePending,
}

/// Result of one finite primary source-124 register/classifier entry.
#[must_use = "a serviced primary epoch must reach fault or scheduler policy"]
pub enum EspHalBluetoothPrimaryInterruptStep {
    /// The shared primary/NRT owner is absent from stable storage.
    Unavailable,
    /// The Controller completed one bounded acknowledged primary disposition.
    Serviced(BluetoothPrimaryInterruptStep),
}

/// Result of one finite default-profile NRT source-133 entry.
#[must_use = "retain the acknowledged NRT epoch"]
pub enum EspHalBluetoothNrtInterruptStep {
    /// The shared primary/NRT owner is absent from stable storage.
    Unavailable,
    /// The default Controller profile acknowledged one opaque NRT epoch.
    Serviced(BluetoothNrtDefaultInterruptEpoch),
}

/// Why the shared primary/NRT owner could not service an interrupt entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothSharedInterruptDispatchError {
    /// The shared owner is absent from process-wide stable storage.
    Unavailable,
}

/// Why scheduler-start interrupt preparation could not borrow stable storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothSchedulerRunInterruptError {
    /// The shared owner is absent from process-wide stable storage.
    Unavailable,
}

/// Why task-side source-127 ownership could not enter or leave stable storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothModemLpTimerStorageError {
    /// No timer owner is present in the process-wide slot.
    Missing,
    /// The stable owner is ready for an IRQ and has no pending software work.
    NotSoftwarePending,
    /// Task work attempted to restore an owner into an occupied slot.
    Occupied,
}

/// Failed task-side rearm retaining the unique ready owner.
#[must_use = "a failed source-127 rearm still owns the timer registers"]
pub struct EspHalBluetoothModemLpTimerRestoreFailure {
    /// Exact stable-storage rejection.
    pub error: EspHalBluetoothModemLpTimerStorageError,
    /// Unchanged ISR-ready owner.
    pub owner: BluetoothModemLpTimerInterruptReadyOwner,
}

impl PublishedEspHalBluetoothInterruptOwners {
    /// Prepare the exact dynamic interrupt groups required before scheduler
    /// event publication while retaining the shared owner in stable storage.
    pub fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, EspHalBluetoothSchedulerRunInterruptError>
    {
        critical_section::with(|critical_section| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            service_stable_owner(&mut slot, |owner| owner.prepare_scheduler_run_interrupts())
                .ok_or(EspHalBluetoothSchedulerRunInterruptError::Unavailable)
        })
    }

    /// Execute one direct post-unlink recheck while retaining the stable
    /// interrupt owner in process-wide storage.
    pub fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut BluetoothControllerHal<'_>,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Result<
        BluetoothSchedulerSoftwareListRemovalJoin,
        BluetoothSchedulerHardwareListHeadEmptyObserved,
    > {
        critical_section::with(|critical_section| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            let Some(interrupts) = slot.as_mut() else {
                return Err(head);
            };
            Ok(controller.recheck_scheduler_software_list_removal(interrupts, head))
        })
    }

    /// Capture, acknowledge and classify one primary source-124 epoch.
    ///
    /// The unique shared register owner remains in its process-wide slot for
    /// later NRT and primary entries. This method publishes no executor wake
    /// and does not make either CPU route live.
    pub fn service_primary_interrupt(&self) -> EspHalBluetoothPrimaryInterruptStep {
        service_bound_bluetooth_primary_interrupt()
    }

    /// Capture and acknowledge one source-133 epoch for the default profile.
    ///
    /// The same shared owner stays published. No synthetic Link-Layer or
    /// executor work is produced by the reviewed default NRT policy.
    pub fn service_nrt_default_interrupt(&self) -> EspHalBluetoothNrtInterruptStep {
        service_bound_bluetooth_nrt_default_interrupt()
    }

    /// Execute at most the source-127 classifier and common register phase.
    ///
    /// A software-pending result leaves the unique owner in stable storage for
    /// task context. Re-entry before task acquisition performs no MMIO.
    pub fn service_modem_lp_timer_interrupt(&self) -> EspHalBluetoothModemLpTimerInterruptStep {
        service_bound_bluetooth_modem_lp_timer_interrupt()
    }

    /// Move source-127 software-pending ownership into task context.
    pub fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<BluetoothModemLpTimerSoftwarePendingOwner, EspHalBluetoothModemLpTimerStorageError>
    {
        critical_section::with(|critical_section| {
            let mut slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            let phase = slot
                .as_ref()
                .map_or(BluetoothModemLpTimerStoragePhase::Missing, |owner| {
                    owner.phase()
                });
            match classify_modem_lp_timer_task_take(phase) {
                BluetoothModemLpTimerTaskTakeAdmission::Missing => {
                    Err(EspHalBluetoothModemLpTimerStorageError::Missing)
                }
                BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending => {
                    Err(EspHalBluetoothModemLpTimerStorageError::NotSoftwarePending)
                }
                BluetoothModemLpTimerTaskTakeAdmission::Acquire => {
                    let Some(StoredBluetoothModemLpTimerOwner::SoftwarePending(owner)) =
                        slot.take()
                    else {
                        return Err(EspHalBluetoothModemLpTimerStorageError::Missing);
                    };
                    Ok(owner)
                }
            }
        })
    }

    /// Return a fully rearmed source-127 owner to stable ISR storage.
    pub fn restore_modem_lp_timer_ready(
        &self,
        owner: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<(), EspHalBluetoothModemLpTimerRestoreFailure> {
        critical_section::with(|critical_section| {
            let mut slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            if !ready_owner_restore_is_admitted(slot.is_some()) {
                return Err(EspHalBluetoothModemLpTimerRestoreFailure {
                    error: EspHalBluetoothModemLpTimerStorageError::Occupied,
                    owner,
                });
            }
            *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(owner));
            Ok(())
        })
    }
}

/// Service one bounded source-124 entry only while a complete route epoch is
/// live.
///
/// This is the narrow entrypoint intended for the installed primary handler.
/// Calling it before the consuming route bind or after successful disable is
/// fail-closed and performs no register access.
fn service_bound_bluetooth_primary_interrupt() -> EspHalBluetoothPrimaryInterruptStep {
    critical_section::with(|critical_section| {
        if BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .is_none_or(|route| !route.live)
        {
            return EspHalBluetoothPrimaryInterruptStep::Unavailable;
        }
        let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        service_stable_owner(&mut slot, step_primary_interrupt).map_or(
            EspHalBluetoothPrimaryInterruptStep::Unavailable,
            EspHalBluetoothPrimaryInterruptStep::Serviced,
        )
    })
}

/// Service one bounded source-133 entry only while a complete route epoch is
/// live.
///
/// This is the narrow entrypoint intended for the installed NRT handler. It
/// acknowledges only the reviewed default disposition and publishes no
/// synthetic controller work.
fn service_bound_bluetooth_nrt_default_interrupt() -> EspHalBluetoothNrtInterruptStep {
    critical_section::with(|critical_section| {
        if BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .is_none_or(|route| !route.live)
        {
            return EspHalBluetoothNrtInterruptStep::Unavailable;
        }
        let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        service_stable_owner(&mut slot, step_nrt_default_interrupt).map_or(
            EspHalBluetoothNrtInterruptStep::Unavailable,
            EspHalBluetoothNrtInterruptStep::Serviced,
        )
    })
}

/// Service one bounded source-127 register step only while the complete route
/// epoch is live.
///
/// The function performs one finite classifier/register disposition. Pending
/// task ownership remains in stable storage, and executor notification stays
/// the responsibility of the Controller interrupt service.
fn service_bound_bluetooth_modem_lp_timer_interrupt() -> EspHalBluetoothModemLpTimerInterruptStep {
    critical_section::with(|critical_section| {
        if BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .is_none_or(|route| !route.live)
        {
            return EspHalBluetoothModemLpTimerInterruptStep::Unavailable;
        }
        let mut slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
        let phase = slot
            .as_ref()
            .map_or(BluetoothModemLpTimerStoragePhase::Missing, |owner| {
                owner.phase()
            });
        match classify_modem_lp_timer_interrupt(phase) {
            BluetoothModemLpTimerInterruptAdmission::Unavailable => {
                EspHalBluetoothModemLpTimerInterruptStep::Unavailable
            }
            BluetoothModemLpTimerInterruptAdmission::AwaitSoftware => {
                EspHalBluetoothModemLpTimerInterruptStep::AwaitingSoftware
            }
            BluetoothModemLpTimerInterruptAdmission::ServiceRegisters => {
                let Some(StoredBluetoothModemLpTimerOwner::Ready(ready)) = slot.take() else {
                    return EspHalBluetoothModemLpTimerInterruptStep::Unavailable;
                };
                match ready.step() {
                    BluetoothModemLpTimerInterruptStep::Spurious(ready) => {
                        *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(ready));
                        EspHalBluetoothModemLpTimerInterruptStep::Spurious
                    }
                    BluetoothModemLpTimerInterruptStep::HandlerPending(pending) => {
                        match pending.step_registers() {
                            BluetoothModemLpTimerHandlerRegisterStep::Rearmed(ready) => {
                                *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(ready));
                                EspHalBluetoothModemLpTimerInterruptStep::Rearmed
                            }
                            BluetoothModemLpTimerHandlerRegisterStep::SoftwarePending(pending) => {
                                *slot = Some(StoredBluetoothModemLpTimerOwner::SoftwarePending(
                                    pending,
                                ));
                                EspHalBluetoothModemLpTimerInterruptStep::SoftwarePending
                            }
                        }
                    }
                }
            }
        }
    })
}

impl BluetoothSchedulerRunInterruptStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothSchedulerRunInterruptError;

    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error> {
        PublishedEspHalBluetoothInterruptOwners::prepare_scheduler_run_interrupts(self)
    }

    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut BluetoothControllerHal<'_>,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Result<
        BluetoothSchedulerSoftwareListRemovalJoin,
        BluetoothSchedulerHardwareListHeadEmptyObserved,
    > {
        PublishedEspHalBluetoothInterruptOwners::recheck_scheduler_software_list_removal(
            self, controller, head,
        )
    }
}

impl BluetoothInterruptOwnerStorage for EspHalBluetoothInterruptStorage {
    type Published = PublishedEspHalBluetoothInterruptOwners;
    type Error = EspHalBluetoothInterruptStorageError;

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
    > {
        critical_section::with(|critical_section| {
            let mut interrupt_slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            let mut timer_slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            match validate_interrupt_storage(interrupt_slot.is_some(), timer_slot.is_some()) {
                Ok(()) => {
                    *interrupt_slot = Some(interrupts);
                    *timer_slot = Some(StoredBluetoothModemLpTimerOwner::Ready(timer));
                    Ok(PublishedEspHalBluetoothInterruptOwners { _private: () })
                }
                Err(error) => Err((error, self, interrupts, timer)),
            }
        })
    }
}

impl BluetoothModemLpTimerSoftwareOwnerStorage for PublishedEspHalBluetoothInterruptOwners {
    type TakeError = EspHalBluetoothModemLpTimerStorageError;
    type RestoreError = EspHalBluetoothModemLpTimerStorageError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<BluetoothModemLpTimerSoftwarePendingOwner, Self::TakeError> {
        PublishedEspHalBluetoothInterruptOwners::take_modem_lp_timer_software_pending(self)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, BluetoothModemLpTimerInterruptReadyOwner)> {
        PublishedEspHalBluetoothInterruptOwners::restore_modem_lp_timer_ready(self, owner)
            .map_err(|failure| (failure.error, failure.owner))
    }
}

impl BluetoothModemLpTimerInterruptDispatchStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothModemLpTimerStorageError;

    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<BluetoothModemLpTimerStableInterruptStep, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_modem_lp_timer_interrupt(self) {
            EspHalBluetoothModemLpTimerInterruptStep::Unavailable => {
                Err(EspHalBluetoothModemLpTimerStorageError::Missing)
            }
            EspHalBluetoothModemLpTimerInterruptStep::AwaitingSoftware => {
                Ok(BluetoothModemLpTimerStableInterruptStep::AwaitingSoftware)
            }
            EspHalBluetoothModemLpTimerInterruptStep::Spurious => {
                Ok(BluetoothModemLpTimerStableInterruptStep::Spurious)
            }
            EspHalBluetoothModemLpTimerInterruptStep::Rearmed => {
                Ok(BluetoothModemLpTimerStableInterruptStep::Rearmed)
            }
            EspHalBluetoothModemLpTimerInterruptStep::SoftwarePending => {
                Ok(BluetoothModemLpTimerStableInterruptStep::SoftwarePending)
            }
        }
    }
}

impl BluetoothSharedInterruptDispatchStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothSharedInterruptDispatchError;

    fn service_primary_interrupt(&self) -> Result<BluetoothPrimaryInterruptStep, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_primary_interrupt(self) {
            EspHalBluetoothPrimaryInterruptStep::Unavailable => {
                Err(EspHalBluetoothSharedInterruptDispatchError::Unavailable)
            }
            EspHalBluetoothPrimaryInterruptStep::Serviced(step) => Ok(step),
        }
    }

    fn service_nrt_default_interrupt(
        &self,
    ) -> Result<BluetoothNrtDefaultInterruptEpoch, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_nrt_default_interrupt(self) {
            EspHalBluetoothNrtInterruptStep::Unavailable => {
                Err(EspHalBluetoothSharedInterruptDispatchError::Unavailable)
            }
            EspHalBluetoothNrtInterruptStep::Serviced(epoch) => Ok(epoch),
        }
    }
}

/// Complete live ESP-HAL Bluetooth interrupt epoch.
///
/// This value joins the stable register-owner publication to exactly one
/// primary, modem-LP timer and NRT CPU route on one core. It exposes no
/// individual route owner and must remain retained for the complete handler
/// lifetime. Dropping it is intentionally fail-stop: routes and stable owners
/// remain installed until board reset rather than being silently reminted.
#[must_use = "the complete Bluetooth interrupt epoch owns all three live CPU routes"]
pub struct BoundEspHalBluetoothInterruptEpoch<'published> {
    published: &'published PublishedEspHalBluetoothInterruptOwners,
    core: Cpu,
}

/// Failed complete-route disable retaining the still-live epoch.
#[must_use = "a rejected Bluetooth route disable retains all live route ownership"]
pub struct EspHalBluetoothInterruptRouteDisableFailure<'published> {
    error: EspHalBluetoothInterruptRouteError,
    epoch: BoundEspHalBluetoothInterruptEpoch<'published>,
}

impl<'published> EspHalBluetoothInterruptRouteDisableFailure<'published> {
    /// Inspect the exact route-set rejection.
    pub const fn error(&self) -> EspHalBluetoothInterruptRouteError {
        self.error
    }

    /// Recover the error and unchanged live route epoch.
    pub fn into_parts(
        self,
    ) -> (
        EspHalBluetoothInterruptRouteError,
        BoundEspHalBluetoothInterruptEpoch<'published>,
    ) {
        (self.error, self.epoch)
    }
}

impl PublishedEspHalBluetoothInterruptOwners {
    /// Bind the complete primary/modem-timer/NRT set to one dispatcher.
    ///
    /// The adapter owns all three exact ESP-HAL handlers and maps them to their
    /// non-interchangeable [`EspHalBluetoothInterruptSource`] values. The
    /// caller therefore supplies one full-controller dispatcher, not three
    /// swappable raw handlers.
    /// A successful full service returns `Serviced`; a fatal storage failure
    /// returns `Quarantine`, which disables only the asserted route before the
    /// adapter-owned handler exits.
    ///
    /// The complete chip/Embassy dispatcher state referenced by `dispatch`
    /// must already be in stable storage before this call. The callback and
    /// live marker are published before the first CPU route is enabled, so an
    /// interrupt observed immediately after binding always sees the complete
    /// dispatcher. A rejected bind leaves this borrowed publication unchanged.
    pub fn bind_routes(
        &self,
        dispatch: fn(EspHalBluetoothInterruptSource) -> EspHalBluetoothInterruptDisposition,
    ) -> Result<BoundEspHalBluetoothInterruptEpoch<'_>, EspHalBluetoothInterruptRouteError> {
        let core = Cpu::current();
        critical_section::with(|critical_section| {
            let mut live_route = BOUND_ROUTE_DISPATCH.borrow_ref_mut(critical_section);
            let mut state = BluetoothInterruptRouteState::from_bound_core(
                live_route.as_ref().map(|route| route.core),
            );
            state.bind(core)?;
            *live_route = Some(BoundRouteDispatch {
                core,
                dispatch,
                live: true,
            });
            interrupt::bind_handler(PRIMARY_INTERRUPT, PRIMARY_HANDLER);
            interrupt::bind_handler(MODEM_LP_TIMER_INTERRUPT, MODEM_LP_TIMER_HANDLER);
            interrupt::bind_handler(NRT_INTERRUPT, NRT_HANDLER);
            Ok::<(), EspHalBluetoothInterruptRouteError>(())
        })?;
        Ok(BoundEspHalBluetoothInterruptEpoch {
            published: self,
            core,
        })
    }
}

impl<'published> BoundEspHalBluetoothInterruptEpoch<'published> {
    /// Prepare scheduler-run interrupt groups through this live epoch's stable
    /// shared owner.
    pub fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, EspHalBluetoothSchedulerRunInterruptError>
    {
        self.published.prepare_scheduler_run_interrupts()
    }

    /// Execute one direct post-unlink recheck through the published owner.
    pub fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut BluetoothControllerHal<'_>,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Result<
        BluetoothSchedulerSoftwareListRemovalJoin,
        BluetoothSchedulerHardwareListHeadEmptyObserved,
    > {
        self.published
            .recheck_scheduler_software_list_removal(controller, head)
    }

    /// Move source-127 software-pending ownership into task context.
    pub fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<BluetoothModemLpTimerSoftwarePendingOwner, EspHalBluetoothModemLpTimerStorageError>
    {
        self.published.take_modem_lp_timer_software_pending()
    }

    /// Return a fully rearmed source-127 owner to this epoch's stable slot.
    pub fn restore_modem_lp_timer_ready(
        &self,
        owner: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<(), EspHalBluetoothModemLpTimerRestoreFailure> {
        self.published.restore_modem_lp_timer_ready(owner)
    }

    /// Disable all three routes on their binding core and end this borrow.
    ///
    /// The timer is closed first so no deadline callback can enter while the
    /// MAC routes are being quiesced. NRT follows because its opaque
    /// acknowledgement path has no controller-side baseline mask. A wrong-core
    /// or inactive-marker rejection returns the complete live epoch unchanged.
    pub fn disable(self) -> Result<(), EspHalBluetoothInterruptRouteDisableFailure<'published>> {
        let current_core = Cpu::current();
        let disabled = critical_section::with(|critical_section| {
            let mut live_route = BOUND_ROUTE_DISPATCH.borrow_ref_mut(critical_section);
            let mut state = BluetoothInterruptRouteState::from_bound_core(
                live_route.as_ref().map(|route| route.core),
            );
            if current_core != self.core {
                return Err(EspHalBluetoothInterruptRouteError::WrongCore);
            }
            state.disable(current_core)?;
            live_route
                .as_mut()
                .expect("a live route state retains its dispatcher")
                .live = false;
            interrupt::disable(self.core, MODEM_LP_TIMER_INTERRUPT);
            interrupt::disable(self.core, NRT_INTERRUPT);
            interrupt::disable(self.core, PRIMARY_INTERRUPT);
            *live_route = None;
            Ok::<(), EspHalBluetoothInterruptRouteError>(())
        });
        match disabled {
            Ok(()) => Ok(()),
            Err(error) => Err(EspHalBluetoothInterruptRouteDisableFailure { error, epoch: self }),
        }
    }
}

impl BluetoothSchedulerRunInterruptStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothSchedulerRunInterruptError;

    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error> {
        BoundEspHalBluetoothInterruptEpoch::prepare_scheduler_run_interrupts(self)
    }

    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut BluetoothControllerHal<'_>,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Result<
        BluetoothSchedulerSoftwareListRemovalJoin,
        BluetoothSchedulerHardwareListHeadEmptyObserved,
    > {
        BoundEspHalBluetoothInterruptEpoch::recheck_scheduler_software_list_removal(
            self, controller, head,
        )
    }
}

impl BluetoothModemLpTimerSoftwareOwnerStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type TakeError = EspHalBluetoothModemLpTimerStorageError;
    type RestoreError = EspHalBluetoothModemLpTimerStorageError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<BluetoothModemLpTimerSoftwarePendingOwner, Self::TakeError> {
        BoundEspHalBluetoothInterruptEpoch::take_modem_lp_timer_software_pending(self)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: BluetoothModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, BluetoothModemLpTimerInterruptReadyOwner)> {
        BoundEspHalBluetoothInterruptEpoch::restore_modem_lp_timer_ready(self, owner)
            .map_err(|failure| (failure.error, failure.owner))
    }
}

impl BluetoothModemLpTimerInterruptDispatchStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothModemLpTimerStorageError;

    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<BluetoothModemLpTimerStableInterruptStep, Self::Error> {
        BluetoothModemLpTimerInterruptDispatchStorage::service_modem_lp_timer_interrupt(
            self.published,
        )
    }
}

impl BluetoothSharedInterruptDispatchStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothSharedInterruptDispatchError;

    fn service_primary_interrupt(&self) -> Result<BluetoothPrimaryInterruptStep, Self::Error> {
        BluetoothSharedInterruptDispatchStorage::service_primary_interrupt(self.published)
    }

    fn service_nrt_default_interrupt(
        &self,
    ) -> Result<BluetoothNrtDefaultInterruptEpoch, Self::Error> {
        BluetoothSharedInterruptDispatchStorage::service_nrt_default_interrupt(self.published)
    }
}
