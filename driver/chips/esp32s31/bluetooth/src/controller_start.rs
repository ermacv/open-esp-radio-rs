//! Controller-output and runtime-timer activation after BLE PHY initialization.

#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptOutputPreparedOwner, BluetoothInterruptRegistersOwner,
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCounterStartedOwner,
    BluetoothModemLpTimerInterruptReadyOwner, BluetoothModemLpTimerSoftwarePendingOwner,
};

#[cfg(target_arch = "riscv32")]
use crate::{
    BluetoothControllerBlePhyEngineInitialized, BluetoothModemLpTimerEventCell,
    BluetoothModemLpTimerEventPublication, BluetoothModemLpTimerExpiration,
    BluetoothModemLpTimerExpirationPending, BluetoothModemLpTimerSoftwareStep,
    BluetoothModemLpTimerSoftwareWork,
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
/// no hard-handler entry is possible from this state.
#[must_use = "published Bluetooth interrupt owners must remain retained through route setup"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerInterruptOwnersPublished<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
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
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    _storage: S,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

/// Failed stable publication retaining the complete Controller and storage.
#[must_use = "failed ISR publication returns every affine owner for inspection or retry"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerInterruptOwnerPublicationFailure<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
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
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerOutputTimerStarted<
        P,
        M,
        MODEM_TIMER_CAPACITY,
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
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerInterruptOwnersPublished<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
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
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerInterruptOwnerPublicationFailure<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
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
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerInterruptOwnersReady<
        P,
        M,
        MODEM_TIMER_CAPACITY,
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
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerInterruptOwnerPublicationFailure<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
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
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerOutputTimerStarted<
        P,
        M,
        MODEM_TIMER_CAPACITY,
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
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerBlePhyEngineInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
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
