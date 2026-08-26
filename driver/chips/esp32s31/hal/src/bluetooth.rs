//! Narrow borrowed HAL capability for ESP32-S31 Bluetooth controller MMIO.
//!
//! The Bluetooth lifecycle retains the unique PAC task owner. Lower layers
//! receive only this finite borrow and named operations; they cannot recover,
//! move or duplicate the underlying register partition.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_pac::{
    BluetoothBasebandInitializationPrerequisite, BluetoothControllerHalInitConfig,
    BluetoothControllerHalInitPrerequisite, BluetoothControllerLatchedTime,
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError, BluetoothInterruptOutputPreparationPrerequisite,
    BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerCounterObservation,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerRegisterObservation,
    BluetoothModemLpTimerInitializationPrerequisite, BluetoothModemLpTimerInstant,
    BluetoothModemLpTimerInterruptEvent, BluetoothModemLpTimerInterruptObservation,
    BluetoothSchedulerDisableBeginError as BluetoothControllerSchedulerDisableBeginError,
    BluetoothSchedulerDisablePrerequisite as BluetoothControllerSchedulerDisablePrerequisite,
};
use open_esp_radio_esp32s31_pac::{
    BluetoothColdRegisters as PacBluetoothColdRegisters, BluetoothInterruptRegisters,
    BluetoothInterruptSetup as PacBluetoothInterruptSetup,
    BluetoothModemLpTimerHandlerPending as PacBluetoothModemLpTimerHandlerPending,
    BluetoothModemLpTimerHandlerRegisterStep as PacBluetoothModemLpTimerHandlerRegisterStep,
    BluetoothModemLpTimerInterruptReady as PacBluetoothModemLpTimerInterruptReady,
    BluetoothModemLpTimerInterruptStep as PacBluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerRegistersPrepared as PacBluetoothModemLpTimerRegistersPrepared,
    BluetoothModemLpTimerSoftwarePending as PacBluetoothModemLpTimerSoftwarePending,
    BluetoothSchedulerDisableBusyObserved as PacSchedulerDisableBusyObserved,
    BluetoothSchedulerDisableIdleObserved as PacSchedulerDisableIdleObserved,
    BluetoothSchedulerDisableRequest as PacSchedulerDisableRequest,
    BluetoothSchedulerDisableStep as PacSchedulerDisableStep,
    BluetoothTaskRegisters as PacBluetoothTaskRegisters,
    BluetoothTaskReuniteError as PacBluetoothTaskReuniteError, RadioHardware,
};

/// Opaque HAL owner for the exclusive Bluetooth route before task/IRQ split.
///
/// The wrapped restricted PAC capability never crosses this boundary. The
/// state performs no MMIO and can therefore be returned losslessly to the
/// protocol-neutral radio root.
#[must_use = "the cold Bluetooth HAL owner retains the complete radio root"]
pub struct BluetoothColdOwner {
    registers: PacBluetoothColdRegisters,
}

impl BluetoothColdOwner {
    /// Enter the exclusive Bluetooth route without touching hardware.
    pub fn from_radio_hardware(hardware: RadioHardware) -> Self {
        Self {
            registers: hardware.into_bluetooth(),
        }
    }

    /// Return the unchanged protocol-neutral radio root.
    pub fn release(self) -> RadioHardware {
        self.registers.release()
    }

    /// Split ordinary task ownership from the inactive controller IRQ bank.
    pub fn separate_interrupt_owner(self) -> (BluetoothTaskOwner, BluetoothInterruptSetupOwner) {
        let (task, interrupts) = self.registers.separate_interrupt_owner();
        (
            BluetoothTaskOwner {
                registers: task,
                reunitable: true,
            },
            BluetoothInterruptSetupOwner {
                registers: interrupts,
                reunitable: true,
            },
        )
    }
}

/// Opaque HAL owner for ordinary Bluetooth task-side controller registers.
#[must_use = "the Bluetooth task owner must be reunited during verified teardown"]
pub struct BluetoothTaskOwner {
    registers: PacBluetoothTaskRegisters,
    reunitable: bool,
}

impl BluetoothTaskOwner {
    /// Reunite a quiescent task with the exact inactive interrupt partition.
    pub fn into_cold(
        self,
        interrupts: BluetoothInterruptSetupOwner,
    ) -> Result<BluetoothColdOwner, BluetoothTaskOwnerReuniteFailure> {
        if !self.reunitable {
            return Err(BluetoothTaskOwnerReuniteFailure {
                task: self,
                interrupts,
                error: BluetoothTaskOwnerReuniteError::HardwareLifecycleNotRestored,
            });
        }
        if !interrupts.reunitable {
            return Err(BluetoothTaskOwnerReuniteFailure {
                task: self,
                interrupts,
                error: BluetoothTaskOwnerReuniteError::InterruptLifecycleNotRestored,
            });
        }
        match self.registers.into_cold(interrupts.registers) {
            Ok(registers) => Ok(BluetoothColdOwner { registers }),
            Err(failure) => {
                let (task, interrupts, error) = failure.into_parts();
                Err(BluetoothTaskOwnerReuniteFailure {
                    task: BluetoothTaskOwner {
                        registers: task,
                        reunitable: false,
                    },
                    interrupts: BluetoothInterruptSetupOwner {
                        registers: interrupts,
                        reunitable: true,
                    },
                    error: match error {
                        PacBluetoothTaskReuniteError::ControllerTimeLatchInFlight => {
                            BluetoothTaskOwnerReuniteError::ControllerTimeLatchInFlight
                        }
                    },
                })
            }
        }
    }

    pub(crate) fn radio_phy_mut(&mut self) -> &mut open_esp_radio_esp32s31_pac::RadioPhyRegisters {
        self.reunitable = false;
        self.registers.radio_phy_mut()
    }

    /// Execute the reviewed BTBB-v2 component after consuming the affine
    /// common-PHY prerequisite supplied by the lifecycle owner.
    #[doc(hidden)]
    pub fn initialize_baseband_v2_arg_one(
        &mut self,
        prerequisite: BluetoothBasebandInitializationPrerequisite,
        gain_parameter: u8,
    ) {
        self.reunitable = false;
        self.registers
            .initialize_baseband_v2_arg_one(prerequisite, gain_parameter);
    }

    /// Execute the complete reviewed 50-operation controller HAL-init body
    /// after consuming its affine external-prerequisite proof.
    #[doc(hidden)]
    pub fn initialize_controller_hal_transaction(
        &mut self,
        prerequisite: BluetoothControllerHalInitPrerequisite,
        config: BluetoothControllerHalInitConfig,
    ) {
        self.reunitable = false;
        self.registers
            .initialize_controller_hal(prerequisite, config);
    }

    /// Apply the exact modem low-power timer register prefix before source 127
    /// is installed.
    ///
    /// This consumes task ownership. The returned state is terminal until the
    /// missing scheduler/HCI software stages, ISR storage and route lifecycle
    /// are implemented; the reviewed teardown cannot restore the cold images.
    #[doc(hidden)]
    pub fn prepare_modem_lp_timer_registers(
        self,
        prerequisite: BluetoothModemLpTimerInitializationPrerequisite,
    ) -> BluetoothModemLpTimerRegistersPreparedOwner {
        BluetoothModemLpTimerRegistersPreparedOwner {
            registers: self
                .registers
                .prepare_modem_lp_timer_registers(prerequisite),
        }
    }
}

/// Opaque HAL owner after the source-127 controller-register prefix.
///
/// It deliberately exposes no task/cold escape or rollback:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerRegistersPreparedOwner;
///
/// fn bypass_remaining_init(prepared: BluetoothModemLpTimerRegistersPreparedOwner) {
///     let _task = prepared.into_task();
/// }
/// ```
#[must_use = "the prepared modem LP-timer owner must continue through route setup"]
pub struct BluetoothModemLpTimerRegistersPreparedOwner {
    registers: PacBluetoothModemLpTimerRegistersPrepared,
}

impl BluetoothModemLpTimerRegistersPreparedOwner {
    /// Transfer the unique task-side partition into stable source-127 ISR
    /// storage before the CPU route is enabled.
    ///
    /// This transition performs no MMIO and exposes no raw PAC owner.
    pub fn stage_for_interrupt(self) -> BluetoothModemLpTimerInterruptReadyOwner {
        BluetoothModemLpTimerInterruptReadyOwner {
            registers: self.registers.stage_for_interrupt(),
        }
    }
}

/// Opaque HAL owner staged for the source-127 hard handler.
#[must_use = "the modem LP-timer interrupt owner must remain in stable ISR storage"]
pub struct BluetoothModemLpTimerInterruptReadyOwner {
    registers: PacBluetoothModemLpTimerInterruptReady,
}

/// Result of one bounded source-127 hard-handler register step.
#[must_use = "retain the ready owner or complete the required software handler"]
pub enum BluetoothModemLpTimerInterruptStep {
    /// `STATUS_0038` was zero and the owner is ready for a later IRQ entry.
    Spurious(BluetoothModemLpTimerInterruptReadyOwner),
    /// The reviewed path requires the common software timer handler.
    HandlerPending(BluetoothModemLpTimerHandlerPendingOwner),
}

/// Opaque HAL owner for the common modem timer handler's register phase.
///
/// No direct rearm or task-owner escape exists. The owner must execute the
/// bounded register step, which either rearms an idle handler or produces the
/// separate fail-closed software-pending state.
#[must_use = "the common modem LP-timer register phase remains pending"]
pub struct BluetoothModemLpTimerHandlerPendingOwner {
    registers: PacBluetoothModemLpTimerHandlerPending,
}

impl BluetoothModemLpTimerHandlerPendingOwner {
    /// Return the exact positional path that selected handler dispatch.
    pub const fn observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.registers.observation()
    }

    /// Execute the bounded register-acknowledgement phase of the common timer
    /// handler without invoking software or an RTOS service.
    pub fn step_registers(self) -> BluetoothModemLpTimerHandlerRegisterStep {
        match self.registers.step_registers() {
            PacBluetoothModemLpTimerHandlerRegisterStep::Rearmed(registers) => {
                BluetoothModemLpTimerHandlerRegisterStep::Rearmed(
                    BluetoothModemLpTimerInterruptReadyOwner { registers },
                )
            }
            PacBluetoothModemLpTimerHandlerRegisterStep::SoftwarePending(registers) => {
                BluetoothModemLpTimerHandlerRegisterStep::SoftwarePending(
                    BluetoothModemLpTimerSoftwarePendingOwner { registers },
                )
            }
        }
    }
}

/// Result of the common handler's bounded register-acknowledgement phase.
#[must_use = "retain the ready owner or complete the required software work"]
pub enum BluetoothModemLpTimerHandlerRegisterStep {
    /// No software work was requested and the interrupt owner is ready again.
    Rearmed(BluetoothModemLpTimerInterruptReadyOwner),
    /// At least one acknowledged state byte requires software work.
    SoftwarePending(BluetoothModemLpTimerSoftwarePendingOwner),
}

/// Opaque HAL owner retained while source-127 software work is pending.
///
/// This owner has no raw PAC escape and cannot be rearmed until the no-RTOS
/// timer runtime supplies the missing software transition and final register
/// read.
#[must_use = "software timer work and the final hardware read remain pending"]
pub struct BluetoothModemLpTimerSoftwarePendingOwner {
    registers: PacBluetoothModemLpTimerSoftwarePending,
}

impl BluetoothModemLpTimerSoftwarePendingOwner {
    /// Return the initial source-127 classifier path.
    pub const fn interrupt_observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.registers.interrupt_observation()
    }

    /// Return the positional state bytes requiring software consequences.
    pub const fn register_observation(&self) -> BluetoothModemLpTimerHandlerRegisterObservation {
        self.registers.register_observation()
    }

    /// Sample one finite positional LP-timer instant and acknowledge a newly
    /// observed rollover without polling.
    pub fn sample_counter(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCounterObservation {
        self.registers.sample_counter(epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        self.registers.disable_compare();
    }

    /// Program one positional deadline and return the exact hardware branch.
    pub fn program_compare(
        &mut self,
        deadline: BluetoothModemLpTimerInstant,
        epoch: BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCompareDisposition {
        self.registers.program_compare(deadline, epoch)
    }

    /// Perform the final fresh handler read and return the ISR-ready owner.
    ///
    /// This seam is hidden because the controller timer state machine must call
    /// it only after every software consequence represented by this owner has
    /// completed.
    #[doc(hidden)]
    pub fn complete_software(self) -> BluetoothModemLpTimerInterruptReadyOwner {
        BluetoothModemLpTimerInterruptReadyOwner {
            registers: self.registers.complete_software(),
        }
    }
}

impl BluetoothModemLpTimerInterruptReadyOwner {
    /// Perform one finite source-127 register classification.
    ///
    /// The method consumes both the unique ISR owner and one affine event. It
    /// never waits, loops, allocates or invokes an RTOS service.
    pub fn step(
        self,
        event: BluetoothModemLpTimerInterruptEvent,
    ) -> BluetoothModemLpTimerInterruptStep {
        match self.registers.step(event) {
            PacBluetoothModemLpTimerInterruptStep::Spurious(registers) => {
                BluetoothModemLpTimerInterruptStep::Spurious(
                    BluetoothModemLpTimerInterruptReadyOwner { registers },
                )
            }
            PacBluetoothModemLpTimerInterruptStep::HandlerPending(registers) => {
                BluetoothModemLpTimerInterruptStep::HandlerPending(
                    BluetoothModemLpTimerHandlerPendingOwner { registers },
                )
            }
        }
    }
}

/// Why the opaque HAL task owner cannot return to cold ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTaskOwnerReuniteError {
    /// A mutable controller or shared-PHY capability was issued and complete
    /// hardware rollback has not returned it to the cold baseline.
    HardwareLifecycleNotRestored,
    /// Interrupt output setup or routing touched hardware and the complete
    /// interrupt-bank baseline has not been restored.
    InterruptLifecycleNotRestored,
    /// A controller-time request still belongs to the task-side worker.
    ControllerTimeLatchInFlight,
}

/// Failed task/IRQ reunion retaining both opaque HAL owners unchanged.
#[must_use = "failed Bluetooth reunion still owns both HAL partitions"]
pub struct BluetoothTaskOwnerReuniteFailure {
    task: BluetoothTaskOwner,
    interrupts: BluetoothInterruptSetupOwner,
    error: BluetoothTaskOwnerReuniteError,
}

impl BluetoothTaskOwnerReuniteFailure {
    /// Return the finite reunion failure reason.
    pub const fn error(&self) -> BluetoothTaskOwnerReuniteError {
        self.error
    }

    /// Recover both retained HAL owners and the failure reason.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothTaskOwner,
        BluetoothInterruptSetupOwner,
        BluetoothTaskOwnerReuniteError,
    ) {
        (self.task, self.interrupts, self.error)
    }
}

impl core::fmt::Debug for BluetoothTaskOwnerReuniteFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothTaskOwnerReuniteFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Opaque inactive owner of the controller interrupt partition.
#[must_use = "the inactive Bluetooth interrupt owner must be staged or reunited"]
pub struct BluetoothInterruptSetupOwner {
    registers: PacBluetoothInterruptSetup,
    reunitable: bool,
}

impl BluetoothInterruptSetupOwner {
    /// Execute the reviewed baseline clear/enable/output preparation.
    ///
    /// The affine prerequisite is constructible only by the lifecycle that
    /// owns completed controller HAL-init and quiescent dynamic sources. The
    /// returned state must remain retained until stable ISR storage is ready.
    pub fn prepare_controller_output(
        self,
        prerequisite: BluetoothInterruptOutputPreparationPrerequisite,
    ) -> BluetoothInterruptOutputPreparedOwner {
        BluetoothInterruptOutputPreparedOwner {
            registers: self.registers.prepare_controller_output(prerequisite),
        }
    }
}

/// Controller IRQ output prepared but not yet transferred into stable ISR
/// storage or bound to a CPU route.
#[must_use = "the prepared Bluetooth output must be staged or released"]
pub struct BluetoothInterruptOutputPreparedOwner {
    registers: open_esp_radio_esp32s31_pac::BluetoothInterruptOutputPrepared,
}

impl BluetoothInterruptOutputPreparedOwner {
    /// Transfer the register partition to the state required by shared ISR
    /// storage before either platform route is enabled.
    pub fn stage_for_cpu_routes(self) -> BluetoothInterruptRegistersOwner {
        BluetoothInterruptRegistersOwner {
            registers: self.registers.stage_for_cpu_routes(),
        }
    }

    /// Execute the reviewed controller-output release transaction after CPU
    /// routes have been removed.
    ///
    /// Dynamic Link-Layer sources must already be quiescent. This transaction
    /// alone is not controller, packet, BTBB, PHY or clock teardown. Even on
    /// the never-routed rollback path, the returned setup owner is marked
    /// non-pristine and cannot reconstruct the neutral hardware root.
    pub fn release_controller_output(self) -> BluetoothInterruptSetupOwner {
        BluetoothInterruptSetupOwner {
            registers: self.registers.release_controller_output(),
            reunitable: false,
        }
    }
}

/// Opaque interrupt-register owner staged for primary and NRT CPU routes.
#[must_use = "the staged Bluetooth interrupt owner must be deactivated"]
pub struct BluetoothInterruptRegistersOwner {
    registers: BluetoothInterruptRegisters,
}

impl BluetoothInterruptRegistersOwner {
    /// Return the register partition to output-prepared ownership after both
    /// CPU routes have been disabled and shared ISR access has ended.
    ///
    /// This distinct state cannot release the controller output yet: dynamic
    /// Link-Layer sources still need their own quiescence proof.
    pub fn deactivate(self) -> BluetoothInterruptOutputAfterRoutesOwner {
        BluetoothInterruptOutputAfterRoutesOwner {
            registers: self.registers.deactivate(),
        }
    }
}

/// Controller interrupt bank recovered from stable ISR storage after both CPU
/// routes were disabled.
///
/// Dynamic Link-Layer sources and output-release ordering are not yet proven,
/// so this state deliberately has no conversion back to setup or cold owners.
#[must_use = "post-route interrupt ownership awaits dynamic-source quiescence"]
pub struct BluetoothInterruptOutputAfterRoutesOwner {
    registers: open_esp_radio_esp32s31_pac::BluetoothInterruptOutputPrepared,
}

/// HAL-owned scheduler disable after its single hardware command was published.
///
/// The task owner cannot be recovered while this value exists. Each
/// [`Self::step`] consumes the state and performs exactly one status
/// observation, which makes cancellation fail-stop and leaves no hidden
/// spin-loop or RTOS dependency.
#[must_use = "the disable command admits one bounded status observation"]
pub struct BluetoothControllerSchedulerDisabling {
    request: PacSchedulerDisableRequest,
}

/// Result of one bounded HAL scheduler-disable observation.
#[must_use = "retain the resulting busy or idle terminal observation"]
pub enum BluetoothControllerSchedulerDisableStep {
    /// One fresh read observed BUSY set. No repeatable polling state escapes.
    BusyObserved(BluetoothControllerSchedulerDisableBusyObserved),
    /// A fresh status read observed the scheduler idle.
    IdleObserved(BluetoothControllerSchedulerDisableIdleObserved),
}

/// HAL ownership after one fresh read observed the BUSY bit set.
///
/// This retains the task without a public escape or another `step`; a later
/// event source must provide an affine recheck permit before it can progress.
#[must_use = "the busy observation retains hardware until a proven recheck edge exists"]
pub struct BluetoothControllerSchedulerDisableBusyObserved {
    _observation: PacSchedulerDisableBusyObserved,
}

/// HAL ownership after one fresh read observed the BUSY bit clear.
///
/// IRQ routing, packet reclamation, BTBB, PHY and clock teardown remain later
/// mandatory edges. This state deliberately claims none of them.
#[must_use = "the idle-bit observation must continue through verified teardown"]
pub struct BluetoothControllerSchedulerDisableIdleObserved {
    _observation: PacSchedulerDisableIdleObserved,
}

/// Failed pre-MMIO scheduler-disable admission retaining the task owner.
#[must_use = "a failed Controller scheduler disable still owns the HAL task owner"]
pub struct BluetoothControllerSchedulerDisableBeginFailure {
    task: BluetoothTaskOwner,
    prerequisite: BluetoothControllerSchedulerDisablePrerequisite,
    error: BluetoothControllerSchedulerDisableBeginError,
}

impl BluetoothControllerSchedulerDisableBeginFailure {
    /// Return the exact finite admission failure reason.
    pub const fn error(&self) -> BluetoothControllerSchedulerDisableBeginError {
        self.error
    }

    /// Recover the unchanged HAL task owner, unconsumed proof and reason.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothTaskOwner,
        BluetoothControllerSchedulerDisablePrerequisite,
        BluetoothControllerSchedulerDisableBeginError,
    ) {
        (self.task, self.prerequisite, self.error)
    }
}

impl core::fmt::Debug for BluetoothControllerSchedulerDisableBeginFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothControllerSchedulerDisableBeginFailure")
            .field("error", &self.error())
            .finish_non_exhaustive()
    }
}

impl BluetoothControllerSchedulerDisabling {
    /// Begin the reviewed scheduler-disable command while the caller retains
    /// the separate interrupt route epoch for later quiescence.
    ///
    /// The affine prerequisite is constructible only at the powered
    /// task-stopping lifecycle edge. It makes no claim about whether CPU routes
    /// are still live. A controller-time latch still in flight is rejected
    /// before the command write and returns the task owner unchanged.
    pub fn begin(
        task: BluetoothTaskOwner,
        prerequisite: BluetoothControllerSchedulerDisablePrerequisite,
    ) -> Result<Self, BluetoothControllerSchedulerDisableBeginFailure> {
        let BluetoothTaskOwner {
            registers,
            reunitable,
        } = task;
        match registers.begin_scheduler_disable(prerequisite) {
            Ok(request) => Ok(Self { request }),
            Err(failure) => {
                let (task, prerequisite, error) = failure.into_parts();
                Err(BluetoothControllerSchedulerDisableBeginFailure {
                    task: BluetoothTaskOwner {
                        registers: task,
                        reunitable,
                    },
                    prerequisite,
                    error,
                })
            }
        }
    }

    /// Perform one fresh status observation after both CPU routes and shared
    /// ISR access have ended, then return immediately.
    ///
    /// No active-route overload exists: current evidence does not establish a
    /// race-free borrow of storage shared with a live ISR. A future route epoch
    /// must supply either quiescence or a value-only ISR observation first.
    pub fn step(
        self,
        interrupts: &mut BluetoothInterruptOutputAfterRoutesOwner,
    ) -> BluetoothControllerSchedulerDisableStep {
        match self
            .request
            .step_after_cpu_routes_disabled(&mut interrupts.registers)
        {
            PacSchedulerDisableStep::BusyObserved(observation) => {
                BluetoothControllerSchedulerDisableStep::BusyObserved(
                    BluetoothControllerSchedulerDisableBusyObserved {
                        _observation: observation,
                    },
                )
            }
            PacSchedulerDisableStep::IdleObserved(observation) => {
                BluetoothControllerSchedulerDisableStep::IdleObserved(
                    BluetoothControllerSchedulerDisableIdleObserved {
                        _observation: observation,
                    },
                )
            }
        }
    }
}

/// Exclusive finite borrow of the Bluetooth controller task-side registers.
///
/// This type deliberately exposes neither `Deref`, a raw PAC accessor nor a
/// constructor. New operations belong here only after their PAC transaction
/// and lifecycle prerequisites are independently bounded.
pub struct BluetoothControllerHal<'registers> {
    registers: &'registers mut PacBluetoothTaskRegisters,
}

impl BluetoothControllerHal<'_> {
    /// Clear the low twenty state bits of all sixteen scheduler entries.
    ///
    /// This is only the reviewed controller-initialization prefix. It does not
    /// establish scheduler, Link Layer or HCI readiness. The borrow proves
    /// exclusive register access, not powered lifecycle state; the caller must
    /// retain the independently established clock/reset prerequisite.
    pub fn clear_scheduler_table_low_bits(&mut self) {
        self.registers.clear_scheduler_table_low_bits();
    }

    /// Publish one controller-time latch request and return immediately.
    ///
    /// The powered lifecycle must first establish a reset/quiescent timer
    /// domain. Entering the Bluetooth ownership route alone performs no reset
    /// and deliberately does not treat an arbitrary hardware bit image as a
    /// fresh request owned by this driver.
    ///
    /// The unique PAC owner remembers the request across HAL borrows. If an
    /// async operation is cancelled before `Ready`, another begin fails closed
    /// and the durable task owner must drain that same request with
    /// [`Self::step_controller_time_latch`] before admitting new work.
    pub fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        self.registers.begin_controller_time_latch()
    }

    /// Perform exactly one observation of the controller-time latch.
    ///
    /// `Waiting` means hardware still owns the request and the caller should
    /// yield until an interrupt or bounded timer event. This method never
    /// loops, registers a waker, allocates or depends on an RTOS.
    pub fn step_controller_time_latch(
        &mut self,
    ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
        self.registers.step_controller_time_latch()
    }

    /// Whether this task owner retains an unfinished latch request.
    ///
    /// This is a diagnostic view for the durable controller task owner. A
    /// cancelled logical operation must be drained before a fresh request is
    /// begun; its sample must not be relabelled as that fresh request.
    pub fn controller_time_latch_in_flight(&self) -> bool {
        self.registers.controller_time_latch_in_flight()
    }
}

mod sealed {
    use super::BluetoothTaskOwner;

    pub trait BluetoothControllerHalBorrow {
        fn bluetooth_task_owner_mut(&mut self) -> &mut BluetoothTaskOwner;
    }

    impl BluetoothControllerHalBorrow for BluetoothTaskOwner {
        fn bluetooth_task_owner_mut(&mut self) -> &mut BluetoothTaskOwner {
            self
        }
    }
}

/// Sealed conversion from the exclusive PAC task owner to one finite
/// controller HAL borrow.
///
/// This conversion proves aliasing only. It deliberately does not manufacture
/// a powered-controller typestate; production operations remain sequenced by
/// the Bluetooth lifecycle owner above this borrow.
///
/// The borrow follows ordinary Rust exclusivity. For example, two simultaneous
/// controller borrows cannot be created:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_hal::BluetoothControllerHalBorrow;
///
/// fn duplicate(owner: &mut impl BluetoothControllerHalBorrow) {
///     let first = owner.borrow_bluetooth_controller();
///     let second = owner.borrow_bluetooth_controller();
///     let _ = (first, second);
/// }
/// ```
#[doc(hidden)]
pub trait BluetoothControllerHalBorrow: sealed::BluetoothControllerHalBorrow {
    /// Borrow the controller registers without exposing their PAC owner.
    fn borrow_bluetooth_controller(&mut self) -> BluetoothControllerHal<'_> {
        let owner = sealed::BluetoothControllerHalBorrow::bluetooth_task_owner_mut(self);
        owner.reunitable = false;
        BluetoothControllerHal {
            registers: &mut owner.registers,
        }
    }
}

impl BluetoothControllerHalBorrow for BluetoothTaskOwner {}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use super::{BluetoothColdOwner, BluetoothControllerHalBorrow, BluetoothTaskOwnerReuniteError};

    #[test]
    fn untouched_task_owner_reconstructs_the_neutral_root() {
        let cold = BluetoothColdOwner::from_radio_hardware(RadioHardware::for_validation());
        let (task, interrupts) = cold.separate_interrupt_owner();
        let hardware = task
            .into_cold(interrupts)
            .expect("an untouched task owner can be reunited")
            .release();

        // Re-entering Wi-Fi proves that the finite HAL borrow neither moved nor
        // duplicated any protocol-neutral owner.
        let _wifi = hardware.into_wifi();
    }

    #[test]
    fn mutable_controller_borrow_arms_fail_stop_reunion() {
        let cold = BluetoothColdOwner::from_radio_hardware(RadioHardware::for_validation());
        let (mut task, interrupts) = cold.separate_interrupt_owner();
        {
            let _controller = task.borrow_bluetooth_controller();
        }

        let failure = match task.into_cold(interrupts) {
            Ok(_) => panic!("hardware rollback is required after a mutable HAL borrow"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothTaskOwnerReuniteError::HardwareLifecycleNotRestored
        );
        let _retained_owners = failure.into_parts();
    }

    #[test]
    fn non_pristine_interrupt_history_blocks_neutral_reunion() {
        let cold = BluetoothColdOwner::from_radio_hardware(RadioHardware::for_validation());
        let (task, mut interrupts) = cold.separate_interrupt_owner();

        // This private state mutation isolates the ownership rule without
        // issuing target MMIO from a host test. The actual prepare/release
        // methods are the only production constructors of this dirty setup.
        interrupts.reunitable = false;

        let failure = match task.into_cold(interrupts) {
            Ok(_) => panic!("interrupt MMIO history requires verified rollback"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothTaskOwnerReuniteError::InterruptLifecycleNotRestored
        );
        let _retained_owners = failure.into_parts();
    }
}
