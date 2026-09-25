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

use crate::bluetooth_route_policy::{
    BluetoothInterruptRouteState, BluetoothModemLpTimerInterruptAdmission,
    BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
    EspHalBluetoothInterruptRetirementError, EspHalBluetoothInterruptRouteError,
    EspHalBluetoothInterruptStorageError, InterruptStorageReservation,
    classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
    ready_owner_restore_is_admitted, retire_interrupt_owner, service_stable_owner,
};

use critical_section::Mutex;

use esp_hal::{
    interrupt::{self, InterruptHandler, Priority},
    peripherals::Interrupt,
    system::Cpu,
};

use oer_esp32s31_bluetooth::{
    interrupt::{
        InterruptOwnerStorage, NrtDefaultInterruptEpoch, PrimaryInterruptStep,
        SharedInterruptDispatchStorage, step_nrt_default_interrupt, step_primary_interrupt,
    },
    modem_lp_timer_queue::ModemLpTimerStableInterruptStep,
    modem_timer::{
        ModemLpTimerInterruptDispatchStorage, ModemLpTimerRetirementStorage,
        ModemLpTimerSoftwareOwnerStorage,
    },
    scheduler::SchedulerRunInterruptStorage,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerRunInterruptsPrepared,
    BluetoothSchedulerSoftwareListRemovalJoin, ControllerHal, InterruptRegistersOwner,
    ModemLpTimerHandlerRegisterStep, ModemLpTimerInterruptReadyOwner, ModemLpTimerInterruptStep,
    ModemLpTimerSoftwarePendingOwner,
};

pub(crate) const PRIMARY_INTERRUPT: Interrupt = Interrupt::BT_MAC;
pub(crate) const MODEM_LP_TIMER_INTERRUPT: Interrupt = Interrupt::MODEM_LP_TIMER;
pub(crate) const NRT_INTERRUPT: Interrupt = Interrupt::BT_MAC_INT1;
const ROUTE_PRIORITY: Priority = Priority::Priority3;

static INTERRUPT_REGISTERS: Mutex<RefCell<Option<InterruptRegistersOwner>>> =
    Mutex::new(RefCell::new(None));
static MODEM_LP_TIMER: Mutex<RefCell<Option<StoredBluetoothModemLpTimerOwner>>> =
    Mutex::new(RefCell::new(None));
static BOUND_ROUTE_DISPATCH: Mutex<RefCell<Option<BoundRouteDispatch>>> =
    Mutex::new(RefCell::new(None));
static STORAGE_RESERVATION: Mutex<RefCell<InterruptStorageReservation>> =
    Mutex::new(RefCell::new(InterruptStorageReservation::new()));

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

type StoredBluetoothModemLpTimerOwner = crate::bluetooth_route_policy::StoredModemTimerOwner<
    ModemLpTimerInterruptReadyOwner,
    ModemLpTimerSoftwarePendingOwner,
>;

/// Stable process-wide slots for both Bluetooth ISR register owners.
///
/// Constructing this value performs no claim. Publication rejects any second
/// value after the first publication, including after both owners are retired.
#[derive(Clone, Copy, Debug, Default)]
pub struct EspHalBluetoothInterruptStorage;

impl EspHalBluetoothInterruptStorage {
    /// Construct an unclaimed reference to the process-wide storage boundary.
    pub const fn new() -> Self {
        Self
    }
}

/// Affine proof of one reserved Bluetooth ISR storage epoch.
///
/// Dropping the lease leaves its owners retained. Explicit retirement may
/// remove the owners, but the reservation remains claimed because old static
/// Controller borrows cannot yet be reclaimed. Only board reset clears it.
#[must_use = "retain the ISR storage reservation through Controller teardown"]
pub struct PublishedEspHalBluetoothInterruptOwners {
    _private: (),
}

/// Actual shared register partition after removal from inactive ISR storage.
///
/// The retired task joins this owner for checked output release; no raw
/// register access or route reactivation is exposed.
#[must_use = "retain the recovered interrupt registers through physical shutdown"]
pub struct RetiredEspHalBluetoothInterruptRegisters {
    _registers: oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
}

/// Actual interrupt bank after idle Controller output release.
/// The setup owner remains non-pristine until the complete powered shutdown.
#[must_use = "retain the released output bank through physical shutdown"]
pub struct ReleasedEspHalBluetoothInterruptRegisters {
    _registers: oer_esp32s31_hal::bluetooth::InterruptOutputReleasedOwner,
}

impl ReleasedEspHalBluetoothInterruptRegisters {
    /// Complete RF close and cold release with the retired task and timer.
    /// The platform must be the reservation joined to this exact HCI epoch.
    /// Once polled, the future must reach a terminal result; failure retains all
    /// hardware, memory and the platform reservation without releasing it.
    pub async fn release_physical<
        'runtime,
        P,
        S,
        D: oer_esp32s31_phy::PhyAsyncDelay,
        const SC: usize,
        const MT: usize,
    >(
        self,
        task: oer_esp32s31_bluetooth::controller::ControllerTaskHciRetired<'runtime, S, SC>,
        timer: oer_esp32s31_bluetooth::modem_timer::ControllerModemTimerRetired<'runtime, S, MT>,
        platform: oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRetiredPlatform<
            'runtime,
            P,
        >,
    ) -> Result<
        oer_esp32s31_bluetooth::controller::ControllerColdReleased<'runtime, P, S, SC, MT>,
        oer_esp32s31_bluetooth::controller::ControllerPhysicalShutdownFailure<
            'runtime,
            P,
            S,
            SC,
            MT,
        >,
    > {
        task.release_physical::<P, D, MT>(timer, self._registers, platform)
            .await
    }
}

impl RetiredEspHalBluetoothInterruptRegisters {
    /// Lend the recovered bank to the same idle Controller's PHY maintenance.
    /// Failure retains the bank, timer and command authority without routing.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit affine owners and observational input"
    )]
    pub fn maintain_phy<
        'a,
        P,
        S,
        D,
        M,
        const SC: usize,
        const MT: usize,
        const H2C: usize,
        const C2H: usize,
        const PC: usize,
    >(
        self,
        task: oer_esp32s31_bluetooth::controller::ControllerIdleCommandTask<'a, S, SC>,
        timer: oer_esp32s31_bluetooth::modem_timer::ControllerModemTimerRetired<'a, S, MT>,
        platform: &mut oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<'a, P>,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
        clock: &mut impl oer_esp32s31_phy::state::client::PhyPllTrackClock,
        observer: impl oer_esp32s31_phy::PhyTargetObserver,
        tracking_deadline: Option<oer_esp32s31_phy::tracking::deadline::TrackingDeadline>,
    ) -> impl core::future::Future<
        Output = Result<
            oer_esp32s31_bluetooth::controller::ControllerPhyMaintained<'a, S, SC, MT>,
            oer_esp32s31_bluetooth::controller::ControllerPhyMaintenanceFailure<'a, S, SC, MT>,
        >,
    >
    where
        S: oer_esp32s31_bluetooth::interrupt::InterruptOwnerRestartStorage
            + oer_esp32s31_bluetooth::modem_timer::ModemLpTimerSoftwareOwnerStorage,
        D: oer_esp32s31_phy::PhyAsyncDelay,
        M: embassy_sync::blocking_mutex::raw::RawMutex,
    {
        // Return the lower future directly: an async forwarding wrapper would
        // retain another move frontier for the complete PHY/IRQ owner graph.
        task.maintain_phy::<P, D, _, M, MT, H2C, C2H, PC>(
            timer,
            self._registers,
            platform,
            controller,
            clock,
            observer,
            tracking_deadline,
        )
    }

    /// Service an admitted ACL window with the actual recovered IRQ bank.
    /// `D`, the PHY clock and `S` must share one monotonic microsecond domain.
    /// The returned connection still retains its bounded restoration obligation.
    pub fn maintain_peripheral_phy<
        'a,
        P,
        S,
        D,
        M,
        const SC: usize,
        const MT: usize,
        const H2C: usize,
        const C2H: usize,
        const PC: usize,
    >(
        self,
        connection: oer_esp32s31_bluetooth::le::peripheral::PeripheralPhyMaintenanceReady<
            'a,
            S,
            SC,
        >,
        timer: oer_esp32s31_bluetooth::modem_timer::ControllerModemTimerRetired<'a, S, MT>,
        platform: &mut oer_esp32s31_bluetooth::resources::platform_retirement::ControllerRuntimePlatform<'a, P>,
        controller: &mut oer_bluetooth_hci::LeControllerCommandEndpoint<'a, M, H2C, C2H, PC>,
        clock: &mut impl oer_esp32s31_phy::state::client::PhyPllTrackClock,
        observer: impl oer_esp32s31_phy::PhyTargetObserver,
    ) -> impl core::future::Future<
        Output = Result<
            oer_esp32s31_bluetooth::controller::ControllerPhyMaintained<
                'a,
                S,
                SC,
                MT,
                oer_esp32s31_bluetooth::le::peripheral::PeripheralPhyMaintenanceRestoring<
                    'a,
                    S,
                    SC,
                >,
            >,
            oer_esp32s31_bluetooth::controller::ControllerPhyMaintenanceFailure<
                'a,
                S,
                SC,
                MT,
                oer_esp32s31_bluetooth::le::peripheral::PeripheralPhyMaintenanceReady<'a, S, SC>,
            >,
        >,
    >
    where
        S: oer_esp32s31_bluetooth::scheduler::SchedulerRunInterruptStorage
            + oer_esp32s31_bluetooth::interrupt::InterruptOwnerRestartStorage
            + oer_esp32s31_bluetooth::modem_timer::ModemLpTimerSoftwareOwnerStorage,
        D: oer_esp32s31_phy::PhyAsyncDelay,
        M: embassy_sync::blocking_mutex::raw::RawMutex,
    {
        connection.maintain_phy::<P, D, _, M, MT, H2C, C2H, PC>(
            timer,
            self._registers,
            platform,
            controller,
            clock,
            observer,
        )
    }

    /// Join the retired task to its post-route register bank for terminal output
    /// release. Rejection preserves both owners and never restores CPU routing.
    pub fn try_release_controller_output<S, const SC: usize>(
        self,
        task: &mut oer_esp32s31_bluetooth::controller::ControllerTaskHciRetired<'_, S, SC>,
    ) -> Result<
        ReleasedEspHalBluetoothInterruptRegisters,
        (
            oer_esp32s31_hal::bluetooth::BluetoothControllerOutputReleaseError,
            Self,
        ),
    > {
        match task.try_release_controller_output(self._registers) {
            Ok(registers) => Ok(ReleasedEspHalBluetoothInterruptRegisters {
                _registers: registers,
            }),
            Err((error, registers)) => Err((
                error,
                Self {
                    _registers: registers,
                },
            )),
        }
    }
}

impl PublishedEspHalBluetoothInterruptOwners {
    /// Take the actual primary/NRT register partition after route removal.
    ///
    /// The timer slot must already be empty. This excludes ISR service under
    /// the same critical section as binding and extraction; it does not prove
    /// task, timer-worker or dynamic-source quiescence. The Controller shutdown
    /// composition must retain its retired task/HCI and drained timer owners.
    /// No register is accessed and the process-wide reservation stays claimed.
    pub fn retire_interrupt_registers_after_routes_disabled(
        &self,
    ) -> Result<RetiredEspHalBluetoothInterruptRegisters, EspHalBluetoothInterruptRetirementError>
    {
        critical_section::with(|cs| {
            retire_interrupt_owner(
                &mut INTERRUPT_REGISTERS.borrow_ref_mut(cs),
                BOUND_ROUTE_DISPATCH.borrow_ref(cs).is_some(),
                MODEM_LP_TIMER.borrow_ref(cs).is_some(),
            )
            .map(|registers| RetiredEspHalBluetoothInterruptRegisters {
                _registers: registers.deactivate(),
            })
        })
    }
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
    Serviced(PrimaryInterruptStep),
}

/// Result of one finite default-profile NRT source-133 entry.
#[must_use = "retain the acknowledged NRT epoch"]
pub enum EspHalBluetoothNrtInterruptStep {
    /// The shared primary/NRT owner is absent from stable storage.
    Unavailable,
    /// The default Controller profile acknowledged one opaque NRT epoch.
    Serviced(NrtDefaultInterruptEpoch),
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
    pub owner: ModemLpTimerInterruptReadyOwner,
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
        controller: &mut ControllerHal<'_>,
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
    ) -> Result<ModemLpTimerSoftwarePendingOwner, EspHalBluetoothModemLpTimerStorageError> {
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
        owner: ModemLpTimerInterruptReadyOwner,
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
                    ModemLpTimerInterruptStep::Spurious(ready) => {
                        *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(ready));
                        EspHalBluetoothModemLpTimerInterruptStep::Spurious
                    }
                    ModemLpTimerInterruptStep::HandlerPending(pending) => {
                        match pending.step_registers() {
                            ModemLpTimerHandlerRegisterStep::Rearmed(ready) => {
                                *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(ready));
                                EspHalBluetoothModemLpTimerInterruptStep::Rearmed
                            }
                            ModemLpTimerHandlerRegisterStep::SoftwarePending(pending) => {
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

impl SchedulerRunInterruptStorage for PublishedEspHalBluetoothInterruptOwners {
    fn monotonic_micros() -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros()
    }

    fn step_scheduler_stop(
        &self,
        controller: &mut ControllerHal<'_>,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopStep,
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    > {
        critical_section::with(|cs| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            match slot.as_mut() {
                Some(interrupts) => Ok(controller.step_scheduler_stop(interrupts, stop)),
                None => Err(stop),
            }
        })
    }

    type Error = EspHalBluetoothSchedulerRunInterruptError;

    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error> {
        PublishedEspHalBluetoothInterruptOwners::prepare_scheduler_run_interrupts(self)
    }

    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut ControllerHal<'_>,
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

impl InterruptOwnerStorage for EspHalBluetoothInterruptStorage {
    type Published = PublishedEspHalBluetoothInterruptOwners;
    type Error = EspHalBluetoothInterruptStorageError;

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
    > {
        critical_section::with(|critical_section| {
            let mut interrupt_slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            let mut timer_slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            match STORAGE_RESERVATION
                .borrow_ref_mut(critical_section)
                .claim(interrupt_slot.is_some(), timer_slot.is_some())
            {
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

impl ModemLpTimerRetirementStorage for PublishedEspHalBluetoothInterruptOwners {
    type RetireError = crate::bluetooth_route_policy::EspHalBluetoothModemLpTimerRetirementError;

    fn take_modem_lp_timer_ready_after_routes_disabled(
        &self,
    ) -> Result<ModemLpTimerInterruptReadyOwner, Self::RetireError> {
        critical_section::with(|cs| {
            // A retained (even quarantined) dispatch prevents extraction. Full
            // same-core disable removes it only after every route is inactive.
            let routes_bound = BOUND_ROUTE_DISPATCH.borrow_ref(cs).is_some();
            crate::bluetooth_route_policy::retire_ready_timer(
                &mut MODEM_LP_TIMER.borrow_ref_mut(cs),
                routes_bound,
            )
        })
    }
}

impl ModemLpTimerSoftwareOwnerStorage for PublishedEspHalBluetoothInterruptOwners {
    type TakeError = EspHalBluetoothModemLpTimerStorageError;
    type RestoreError = EspHalBluetoothModemLpTimerStorageError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Self::TakeError> {
        PublishedEspHalBluetoothInterruptOwners::take_modem_lp_timer_software_pending(self)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, ModemLpTimerInterruptReadyOwner)> {
        PublishedEspHalBluetoothInterruptOwners::restore_modem_lp_timer_ready(self, owner)
            .map_err(|failure| (failure.error, failure.owner))
    }
}

impl ModemLpTimerInterruptDispatchStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothModemLpTimerStorageError;

    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerStableInterruptStep, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_modem_lp_timer_interrupt(self) {
            EspHalBluetoothModemLpTimerInterruptStep::Unavailable => {
                Err(EspHalBluetoothModemLpTimerStorageError::Missing)
            }
            EspHalBluetoothModemLpTimerInterruptStep::AwaitingSoftware => {
                Ok(ModemLpTimerStableInterruptStep::AwaitingSoftware)
            }
            EspHalBluetoothModemLpTimerInterruptStep::Spurious => {
                Ok(ModemLpTimerStableInterruptStep::Spurious)
            }
            EspHalBluetoothModemLpTimerInterruptStep::Rearmed => {
                Ok(ModemLpTimerStableInterruptStep::Rearmed)
            }
            EspHalBluetoothModemLpTimerInterruptStep::SoftwarePending => {
                Ok(ModemLpTimerStableInterruptStep::SoftwarePending)
            }
        }
    }
}

impl SharedInterruptDispatchStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothSharedInterruptDispatchError;

    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptStep, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_primary_interrupt(self) {
            EspHalBluetoothPrimaryInterruptStep::Unavailable => {
                Err(EspHalBluetoothSharedInterruptDispatchError::Unavailable)
            }
            EspHalBluetoothPrimaryInterruptStep::Serviced(step) => Ok(step),
        }
    }

    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error> {
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
    /// Both register owners must be in stable storage; a timer held by task
    /// work or retirement must be restored before reactivation.
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
            crate::bluetooth_route_policy::validate_route_owners(
                INTERRUPT_REGISTERS.borrow_ref(critical_section).is_some(),
                MODEM_LP_TIMER.borrow_ref(critical_section).is_some(),
            )?;
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
        controller: &mut ControllerHal<'_>,
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
    ) -> Result<ModemLpTimerSoftwarePendingOwner, EspHalBluetoothModemLpTimerStorageError> {
        self.published.take_modem_lp_timer_software_pending()
    }

    /// Return a fully rearmed source-127 owner to this epoch's stable slot.
    pub fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
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

impl SchedulerRunInterruptStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    fn monotonic_micros() -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros()
    }

    fn step_scheduler_stop(
        &self,
        controller: &mut ControllerHal<'_>,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopStep,
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    > {
        critical_section::with(|cs| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            match slot.as_mut() {
                Some(interrupts) => Ok(controller.step_scheduler_stop(interrupts, stop)),
                None => Err(stop),
            }
        })
    }

    type Error = EspHalBluetoothSchedulerRunInterruptError;

    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error> {
        BoundEspHalBluetoothInterruptEpoch::prepare_scheduler_run_interrupts(self)
    }

    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut ControllerHal<'_>,
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

impl ModemLpTimerSoftwareOwnerStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type TakeError = EspHalBluetoothModemLpTimerStorageError;
    type RestoreError = EspHalBluetoothModemLpTimerStorageError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Self::TakeError> {
        BoundEspHalBluetoothInterruptEpoch::take_modem_lp_timer_software_pending(self)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, ModemLpTimerInterruptReadyOwner)> {
        BoundEspHalBluetoothInterruptEpoch::restore_modem_lp_timer_ready(self, owner)
            .map_err(|failure| (failure.error, failure.owner))
    }
}

impl ModemLpTimerInterruptDispatchStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothModemLpTimerStorageError;

    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerStableInterruptStep, Self::Error> {
        ModemLpTimerInterruptDispatchStorage::service_modem_lp_timer_interrupt(self.published)
    }
}

impl SharedInterruptDispatchStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothSharedInterruptDispatchError;

    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptStep, Self::Error> {
        SharedInterruptDispatchStorage::service_primary_interrupt(self.published)
    }

    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error> {
        SharedInterruptDispatchStorage::service_nrt_default_interrupt(self.published)
    }
}

impl oer_esp32s31_bluetooth::interrupt::InterruptOwnerRestartStorage
    for PublishedEspHalBluetoothInterruptOwners
{
    type RestartError = EspHalBluetoothInterruptStorageError;
    fn restore_initialized_interrupt_owners(
        &self,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        (),
        (
            Self::RestartError,
            InterruptRegistersOwner,
            ModemLpTimerInterruptReadyOwner,
        ),
    > {
        critical_section::with(|cs| {
            let mut irq_slot = INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            let mut timer_slot = MODEM_LP_TIMER.borrow_ref_mut(cs);
            if let Err(error) = STORAGE_RESERVATION.borrow_ref(cs).admit_restore(
                BOUND_ROUTE_DISPATCH.borrow_ref(cs).is_some(),
                irq_slot.is_some(),
                timer_slot.is_some(),
            ) {
                return Err((error, interrupts, timer));
            }
            *irq_slot = Some(interrupts);
            *timer_slot = Some(StoredBluetoothModemLpTimerOwner::Ready(timer));
            Ok(())
        })
    }
}
