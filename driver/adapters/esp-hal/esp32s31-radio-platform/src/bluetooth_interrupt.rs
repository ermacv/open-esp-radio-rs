//! Typed ESP-HAL routing primitives for all three Bluetooth interrupts.
//!
//! Stable publication is deliberately separate from a live interrupt epoch.
//! Both unique HAL owners are installed atomically before any CPU route can be
//! enabled. All three finite Controller dispositions can now reuse those
//! stable owners, but handler notification, selector-6 recovery and the
//! scheduler-list drain still block a live route epoch.

#![forbid(unsafe_code)]
#![allow(
    dead_code,
    reason = "typed route primitives await stable ISR storage and complete three-source dispatch"
)]

use core::cell::RefCell;

use critical_section::Mutex;
use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    peripherals::Interrupt,
    system::Cpu,
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothCpuInterruptRoutePolicy, BluetoothInterruptOwnerStorage,
    BluetoothModemLpTimerInterruptDispatchStorage, BluetoothModemLpTimerSoftwareOwnerStorage,
    BluetoothModemLpTimerStableInterruptStep, BluetoothNrtDefaultInterruptEpoch,
    BluetoothPrimaryInterruptStep, BluetoothSharedInterruptDispatchStorage,
    step_nrt_default_interrupt, step_primary_interrupt,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptRegistersOwner, BluetoothModemLpTimerHandlerRegisterStep,
    BluetoothModemLpTimerInterruptReadyOwner, BluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerSoftwarePendingOwner,
};

use crate::bluetooth_route_policy::{
    BluetoothInterruptRouteError, BluetoothModemLpTimerInterruptAdmission,
    BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
    EspHalBluetoothInterruptStorageError, REQUIRED_PRIORITY_LEVEL,
    classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
    ready_owner_restore_is_admitted, service_stable_owner, validate_interrupt_storage,
    validate_quiesce_core, validate_route_priorities,
};

pub(crate) const PRIMARY_INTERRUPT: Interrupt = Interrupt::BT_MAC;
pub(crate) const MODEM_LP_TIMER_INTERRUPT: Interrupt = Interrupt::MODEM_LP_TIMER;
pub(crate) const NRT_INTERRUPT: Interrupt = Interrupt::BT_MAC_INT1;
const ROUTE_PRIORITY: Priority = Priority::Priority3;

// Fail compilation if either the reviewed chip policy or the generated PAC
// identity moves independently. No raw interrupt number reaches ESP-HAL.
const _: () = assert!(
    PRIMARY_INTERRUPT as u16 == BluetoothCpuInterruptRoutePolicy::PRIMARY.source().number()
);
const _: () = assert!(
    MODEM_LP_TIMER_INTERRUPT as u16
        == BluetoothCpuInterruptRoutePolicy::MODEM_LP_TIMER
            .source()
            .number()
);
const _: () =
    assert!(NRT_INTERRUPT as u16 == BluetoothCpuInterruptRoutePolicy::NRT.source().number());
const _: () = assert!(BluetoothCpuInterruptRoutePolicy::PRIMARY.priority_level() == 3);
const _: () = assert!(BluetoothCpuInterruptRoutePolicy::MODEM_LP_TIMER.priority_level() == 3);
const _: () = assert!(BluetoothCpuInterruptRoutePolicy::NRT.priority_level() == 3);
const _: () = assert!(ROUTE_PRIORITY as u8 == REQUIRED_PRIORITY_LEVEL);

static INTERRUPT_REGISTERS: Mutex<RefCell<Option<BluetoothInterruptRegistersOwner>>> =
    Mutex::new(RefCell::new(None));
static MODEM_LP_TIMER: Mutex<RefCell<Option<StoredBluetoothModemLpTimerOwner>>> =
    Mutex::new(RefCell::new(None));

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
    /// Capture, acknowledge and classify one primary source-124 epoch.
    ///
    /// The unique shared register owner remains in its process-wide slot for
    /// later NRT and primary entries. This method publishes no executor wake
    /// and does not make either CPU route live.
    pub fn service_primary_interrupt(&self) -> EspHalBluetoothPrimaryInterruptStep {
        critical_section::with(|critical_section| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            service_stable_owner(&mut slot, step_primary_interrupt).map_or(
                EspHalBluetoothPrimaryInterruptStep::Unavailable,
                EspHalBluetoothPrimaryInterruptStep::Serviced,
            )
        })
    }

    /// Capture and acknowledge one source-133 epoch for the default profile.
    ///
    /// The same shared owner stays published. No synthetic Link-Layer or
    /// executor work is produced by the reviewed default NRT policy.
    pub fn service_nrt_default_interrupt(&self) -> EspHalBluetoothNrtInterruptStep {
        critical_section::with(|critical_section| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            service_stable_owner(&mut slot, step_nrt_default_interrupt).map_or(
                EspHalBluetoothNrtInterruptStep::Unavailable,
                EspHalBluetoothNrtInterruptStep::Serviced,
            )
        })
    }

    /// Execute at most the source-127 classifier and common register phase.
    ///
    /// A software-pending result leaves the unique owner in stable storage for
    /// task context. Re-entry before task acquisition performs no MMIO.
    pub fn service_modem_lp_timer_interrupt(&self) -> EspHalBluetoothModemLpTimerInterruptStep {
        critical_section::with(|critical_section| {
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
                                BluetoothModemLpTimerHandlerRegisterStep::SoftwarePending(
                                    pending,
                                ) => {
                                    *slot = Some(
                                        StoredBluetoothModemLpTimerOwner::SoftwarePending(pending),
                                    );
                                    EspHalBluetoothModemLpTimerInterruptStep::SoftwarePending
                                }
                            }
                        }
                    }
                }
            }
        })
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

/// Proof that all Bluetooth routes were bound on the same CPU core.
///
/// The value must stay owned by the future interrupt epoch until all routes
/// have been disabled. It intentionally exposes no individual-line teardown.
#[must_use = "all Bluetooth CPU routes must be disabled before ISR storage is recovered"]
pub struct BoundBluetoothInterruptRoutes {
    core: Cpu,
}

/// Bind the complete primary/modem-timer/NRT set using typed PAC identities.
///
/// Every handler priority is checked before the first interrupt is enabled,
/// so an invalid set leaves the interrupt matrix unchanged. The caller must
/// already have published the shared register owner and must retain it until
/// [`BoundBluetoothInterruptRoutes::disable`] returns successfully.
pub(crate) fn bind(
    primary_handler: InterruptHandler,
    modem_lp_timer_handler: InterruptHandler,
    nrt_handler: InterruptHandler,
) -> Result<BoundBluetoothInterruptRoutes, BluetoothInterruptRouteError> {
    validate_route_priorities(
        primary_handler.priority() as u8,
        modem_lp_timer_handler.priority() as u8,
        nrt_handler.priority() as u8,
    )?;

    let core = Cpu::current();
    interrupt::bind_handler(PRIMARY_INTERRUPT, primary_handler);
    interrupt::bind_handler(MODEM_LP_TIMER_INTERRUPT, modem_lp_timer_handler);
    interrupt::bind_handler(NRT_INTERRUPT, nrt_handler);
    Ok(BoundBluetoothInterruptRoutes { core })
}

impl BoundBluetoothInterruptRoutes {
    /// Disable all routes on the core where the set was installed.
    ///
    /// The timer is closed first so no deadline callback can enter while the
    /// MAC routes are being quiesced. NRT follows because its opaque
    /// acknowledgement path has no controller-side baseline mask. Teardown on
    /// any other core returns the intact route owner without changing the
    /// matrix. After success, no handler can begin a new same-core epoch and
    /// the caller may recover shared storage.
    pub(crate) fn disable(
        self,
    ) -> Result<(), (BluetoothInterruptRouteError, BoundBluetoothInterruptRoutes)> {
        if let Err(error) = validate_quiesce_core(Cpu::current() == self.core) {
            return Err((error, self));
        }
        interrupt::disable(self.core, MODEM_LP_TIMER_INTERRUPT);
        interrupt::disable(self.core, NRT_INTERRUPT);
        interrupt::disable(self.core, PRIMARY_INTERRUPT);
        Ok(())
    }
}
