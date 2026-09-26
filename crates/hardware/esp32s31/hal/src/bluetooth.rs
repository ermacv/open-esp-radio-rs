//! Narrow borrowed HAL capability for ESP32-S31 Bluetooth controller MMIO.
//!
//! The Bluetooth lifecycle retains the unique PAC task owner. Lower layers
//! receive only this finite borrow and named operations; they cannot recover,
//! move or duplicate the underlying register partition.

#![deny(unsafe_code)]

use oer_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothDirectionFindingDisabledBaselinePrepared,
    BluetoothInterruptRegisters, BluetoothInterruptSetup as PacBluetoothInterruptSetup,
    BluetoothLowPowerClockObservation, BluetoothMemoryListPointerImage,
    BluetoothMemoryListSelector, BluetoothMemoryListSlot, BluetoothModemLpTimerRegisters,
    BluetoothPrimaryInterruptEpoch, BluetoothTaskRegisters as PacBluetoothTaskRegisters,
    ModemSysconBluetoothObservation, PlatformClockPowerObservation, SharedModemClockObservation,
};

use crate::{
    clock::BluetoothClocks,
    phy::restore::PhyRestoreSlot,
    root::{BluetoothRoute, RadioHardware, RadioPhyReleaseError, RetainedWifi},
};

mod controller_time;
mod scheduler_stop;
mod shutdown;

use controller_time::ControllerTimeLatch;
pub use controller_time::{
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchRequest,
    BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError,
};
pub use scheduler_stop::{BluetoothSchedulerStop, BluetoothSchedulerStopStep};
pub use shutdown::{BluetoothPhysicalReleaseError, BluetoothPhysicalReleaseFailure};

pub use oer_esp32s31_pac::{
    BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
    BluetoothControllerOutputReleaseError, BluetoothLowPowerRuntimeControlObservation,
    BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerCounterObservation,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerRegisterObservation,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    BluetoothNrtInterruptAcknowledged, BluetoothPhyEnvironmentAddress,
    BluetoothPhyEnvironmentAddressError, BluetoothPhyRegisterInitInputs,
    BluetoothScanStartPublished, BluetoothSchedulerExecutionLockDisposition,
    BluetoothSchedulerExecutionLockPublished, BluetoothSchedulerExecutionLockRequest,
    BluetoothSchedulerExecutionModifyDisposition, BluetoothSchedulerExecutionModifyPublished,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListObservation,
    BluetoothSchedulerFinishedListPop, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerHardwareListHeadError,
    BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerLockModifyObservation,
    BluetoothSchedulerLockModifyPublished, BluetoothSchedulerLockModifyRequest,
    BluetoothSchedulerLockModifyTaskObservation, BluetoothSchedulerReferenceCleared,
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerRunEventPublished,
    BluetoothSchedulerRunInterruptsPrepared, BluetoothSchedulerSoftwareListRemovalIdle,
    BluetoothSchedulerSoftwareListRemovalInterruptStep, BluetoothSchedulerSoftwareListRemovalJoin,
    BluetoothSchedulerSoftwareListRemovalReady, BluetoothSchedulerStopped,
    BluetoothSchedulerStoppedHeadRetirement, BluetoothSchedulerStoppedItem,
    BluetoothSchedulerWorkObservation,
};

/// Opaque HAL owner for the exclusive Bluetooth route before task/IRQ split.
///
/// The wrapped restricted PAC capability never crosses this boundary. The
/// state performs no MMIO and can therefore be returned losslessly to the
/// protocol-neutral radio root.
#[must_use = "the cold Bluetooth HAL owner retains the complete radio root"]
pub struct ColdOwner {
    task: PacBluetoothTaskRegisters,
    modem_lp_timer: BluetoothModemLpTimerRegisters,
    interrupts: PacBluetoothInterruptSetup,
    retained: RetainedWifi,
    clocks: BluetoothClocks,
    phy_restore: PhyRestoreSlot,
}

/// Failed cold Bluetooth release retaining the complete HAL owner.
#[must_use = "failed Bluetooth release still owns the complete cold route"]
pub struct ColdOwnerReleaseFailure {
    owner: ColdOwner,
    error: RadioPhyReleaseError,
}

impl ColdOwnerReleaseFailure {
    pub const fn error(&self) -> RadioPhyReleaseError {
        self.error
    }

    pub fn into_parts(self) -> (ColdOwner, RadioPhyReleaseError) {
        (self.owner, self.error)
    }
}

impl core::fmt::Debug for ColdOwnerReleaseFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ColdOwnerReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl ColdOwner {
    /// Capture the shared power baseline before Bluetooth retains any clocks.
    #[doc(hidden)]
    pub fn prepare_shared_power_epoch(&mut self) {
        self.clocks.prepare_power_epoch(self.task.radio_phy());
    }

    /// Enter the exclusive Bluetooth route without touching hardware.
    pub fn from_radio_hardware(hardware: RadioHardware) -> Self {
        let BluetoothRoute {
            task,
            modem_lp_timer,
            interrupts,
            retained,
        } = hardware.into_bluetooth();
        Self {
            task,
            modem_lp_timer,
            interrupts,
            retained,
            clocks: BluetoothClocks::default(),
            phy_restore: PhyRestoreSlot::default(),
        }
    }

    /// Return the unchanged protocol-neutral radio root.
    ///
    /// Release drops every retained clock lease, restores the cold-power
    /// baseline captured by [`Self::prepare_shared_power_epoch`] and only then
    /// reconstructs the neutral root.
    ///
    /// # Errors
    ///
    /// Returns [`ColdOwnerReleaseFailure`] retaining this owner while
    /// TX-DC PWDET, TX-IQ, RX-DCO, or Bluetooth TX-power control still awaits
    /// restoration, or when a cold-power baseline fails readback.
    pub fn release(mut self) -> Result<RadioHardware, ColdOwnerReleaseFailure> {
        if let Err(error) = crate::root::check_phy_restore_complete(&self.phy_restore) {
            return Err(ColdOwnerReleaseFailure { owner: self, error });
        }
        let phy = self.task.radio_phy_mut();
        self.clocks.release_all(phy);
        if let Err(checkpoint) = self.clocks.restore_power_epoch(phy) {
            return Err(ColdOwnerReleaseFailure {
                owner: self,
                error: RadioPhyReleaseError::WifiPowerRestore(checkpoint),
            });
        }
        Ok(RadioHardware::from_bluetooth(
            self.task,
            self.modem_lp_timer,
            self.interrupts,
            self.retained,
        ))
    }

    #[doc(hidden)]
    pub fn prepare_shared_modem_clock_map(&mut self) {
        self.task.prepare_shared_modem_clock_map();
    }

    #[doc(hidden)]
    pub fn retain_coexistence_clock(&mut self) {
        self.clocks.retain_coexistence(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn release_coexistence_clock(&mut self) {
        self.clocks.release_coexistence(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn retain_main_xtal_bluetooth_low_power_clock(&mut self) {
        self.clocks
            .retain_main_xtal_low_power_timer(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn release_bluetooth_low_power_timer(&mut self) {
        self.clocks
            .release_low_power_timer(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn bluetooth_shared_clock_observation(
        &self,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        self.task.bluetooth_shared_clock_observation()
    }

    #[doc(hidden)]
    pub fn retain_platform_pll_source(&mut self) {
        self.clocks
            .retain_platform_pll_source(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn release_platform_pll_source(&mut self) {
        self.clocks
            .release_platform_pll_source(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.task.platform_clock_power_observation()
    }

    #[doc(hidden)]
    pub fn prepare_modem_syscon_clock_map(&mut self) {
        self.task.radio_phy_mut().prepare_modem_syscon_clock_map();
    }

    #[doc(hidden)]
    pub fn reset_modem_syscon_bluetooth_domains(&mut self) {
        self.task
            .radio_phy_mut()
            .reset_bluetooth_controller_domains();
    }

    #[doc(hidden)]
    pub fn modem_syscon_bluetooth_observation(&self) -> ModemSysconBluetoothObservation {
        self.task.radio_phy().bluetooth_clock_observation()
    }

    #[doc(hidden)]
    pub fn retain_modem_syscon_controller_clocks(&mut self) {
        self.clocks
            .retain_syscon_controller_clocks(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn retain_modem_syscon_apb_clocks(&mut self) {
        self.clocks
            .retain_syscon_apb_clocks(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn release_modem_syscon_apb_clocks(&mut self) {
        self.clocks
            .release_syscon_apb_clocks(self.task.radio_phy_mut());
    }

    #[doc(hidden)]
    pub fn release_modem_syscon_controller_clocks(&mut self) {
        self.clocks
            .release_syscon_controller_clocks(self.task.radio_phy_mut());
    }

    /// Split ordinary task ownership from the inactive controller IRQ bank.
    ///
    /// This conversion performs no MMIO and does not claim that the hardware
    /// interrupt route has been configured or enabled.
    pub fn separate_interrupt_owner(self) -> (TaskOwner, InterruptSetupOwner) {
        (
            TaskOwner {
                registers: self.task,
                modem_lp_timer: Some(self.modem_lp_timer),
                retained: self.retained,
                clocks: self.clocks,
                phy_restore: self.phy_restore,
                time_latch: ControllerTimeLatch::new(),
                reunitable: true,
            },
            InterruptSetupOwner {
                registers: self.interrupts,
                reunitable: true,
            },
        )
    }
}

/// Opaque HAL owner for ordinary Bluetooth task-side controller registers.
#[must_use = "the Bluetooth task owner must be reunited during verified teardown"]
pub struct TaskOwner {
    registers: PacBluetoothTaskRegisters,
    modem_lp_timer: Option<BluetoothModemLpTimerRegisters>,
    retained: RetainedWifi,
    clocks: BluetoothClocks,
    phy_restore: PhyRestoreSlot,
    time_latch: ControllerTimeLatch,
    reunitable: bool,
}

impl TaskOwner {
    /// Complete the physical release after last-client RF close and temperature
    /// power-down. The output token proves post-route scheduler quiescence; the
    /// outer Controller retains the drained timer and every published memory
    /// owner until reset and checked clock restoration return the neutral root.
    #[doc(hidden)]
    pub fn release_after_phy_close(
        self,
        output: InterruptOutputReleasedOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<RadioHardware, BluetoothPhysicalReleaseFailure> {
        shutdown::release_after_phy_close(self, output.registers, timer.timer)
    }
    /// Establish the shared modem/PHY power, reset and calibration clocks.
    ///
    /// Call once before common PHY registration on the exclusive cold route.
    /// The inactive Wi-Fi partition remains retained; no Wi-Fi MAC clock or
    /// protocol role is started. Success and failure both retain the PHY I2C
    /// lease and revoke cold reunion until physical teardown is implemented.
    #[doc(hidden)]
    pub fn prepare_common_phy_power(&mut self) -> Result<(), crate::power::PowerError> {
        crate::power::execute_bluetooth_owned(
            &mut self.reunitable,
            &mut crate::power::RoutePower {
                phy: self.registers.radio_phy_mut(),
                leases: self.clocks.shared_mut(),
            },
        )
    }

    /// Reunite a quiescent task with the exact inactive interrupt partition.
    pub fn into_cold(
        self,
        interrupts: InterruptSetupOwner,
    ) -> Result<ColdOwner, TaskOwnerReuniteFailure> {
        if !self.reunitable {
            return Err(TaskOwnerReuniteFailure {
                task: self,
                interrupts,
                error: TaskOwnerReuniteError::HardwareLifecycleNotRestored,
            });
        }
        if !interrupts.reunitable {
            return Err(TaskOwnerReuniteFailure {
                task: self,
                interrupts,
                error: TaskOwnerReuniteError::InterruptLifecycleNotRestored,
            });
        }
        let error = if self.time_latch.in_flight() {
            Some(TaskOwnerReuniteError::ControllerTimeLatchInFlight)
        } else if self.modem_lp_timer.is_none() {
            Some(TaskOwnerReuniteError::ModemLpTimerOwnerSeparated)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(TaskOwnerReuniteFailure {
                task: TaskOwner {
                    reunitable: false,
                    ..self
                },
                interrupts,
                error,
            });
        }
        let TaskOwner {
            registers,
            modem_lp_timer,
            retained,
            clocks,
            phy_restore,
            time_latch: _,
            reunitable: _,
        } = self;
        Ok(ColdOwner {
            task: registers,
            modem_lp_timer: modem_lp_timer
                .expect("an unseparated task owner retains its modem LP-timer partition"),
            interrupts: interrupts.registers,
            retained,
            clocks,
            phy_restore,
        })
    }

    /// Borrow the shared PHY and route restore slot, arming fail-stop reunion.
    pub(crate) fn phy_parts_mut(
        &mut self,
    ) -> (
        &mut oer_esp32s31_pac::RadioPhyRegisters,
        &mut PhyRestoreSlot,
    ) {
        self.reunitable = false;
        (self.registers.radio_phy_mut(), &mut self.phy_restore)
    }

    #[cfg(test)]
    pub(crate) fn phy_restore_mut(&mut self) -> &mut PhyRestoreSlot {
        &mut self.phy_restore
    }

    /// Execute the reviewed BTBB-v2 component for the lifecycle owner that
    /// retains the completed common-PHY state.
    ///
    /// # Safety
    ///
    /// The caller must retain enabled Bluetooth clocks and the completed
    /// common-PHY owner for this exact task partition.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain the external common-PHY and powered-clock owners"
    )]
    pub unsafe fn initialize_baseband_v2_arg_one(&mut self, gain_parameter: u8) {
        self.reunitable = false;
        self.registers
            .initialize_baseband_v2_arg_one(gain_parameter);
    }

    /// Execute the complete reviewed BLE base-stack task-enable hardware transaction.
    ///
    /// # Safety
    ///
    /// The caller must retain the completed common-PHY and BTBB owners, the
    /// initialized source-owned controller software, an inactive IRQ route,
    /// and both pointed storage objects for every hardware consumer.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain external lifecycle and pointed-storage owners"
    )]
    pub unsafe fn enable_ble_base_stack_hardware(
        &mut self,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        self.reunitable = false;
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe {
            self.registers.enable_ble_base_stack_hardware(inputs);
        }
    }

    /// Execute the complete reviewed 50-operation controller HAL-init body
    /// at the upper controller lifecycle's verified transition.
    ///
    /// # Safety
    ///
    /// The caller must retain enabled controller clocks, the selected
    /// controller-SRAM prefix and the inactive interrupt bank. It must not
    /// infer scheduler, PHY, BTBB, Link-Layer or HCI readiness from return.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain the external clock, SRAM-prefix and interrupt owners"
    )]
    pub unsafe fn initialize_controller_hal_transaction(
        &mut self,
        config: BluetoothControllerHalInitConfig,
    ) {
        self.reunitable = false;
        self.registers.initialize_controller_hal(config);
    }

    /// Apply the exact modem low-power timer register prefix before source 127
    /// is installed.
    ///
    /// This extracts only the generated timer partition. Controller task
    /// ownership remains available for scheduler and Link-Layer progress while
    /// source 127 retains exclusive timer-register authority.
    ///
    /// # Safety
    ///
    /// The caller must retain initialized controller/timer software and an
    /// inactive source-127 CPU route for this exact task owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain the initialized timer software and inactive source-127 route"
    )]
    pub unsafe fn prepare_modem_lp_timer_registers(
        &mut self,
    ) -> Result<ModemLpTimerRegistersPreparedOwner, BluetoothModemLpTimerOwnerError> {
        self.reunitable = false;
        // Extract the partition before the first MMIO effect: cancellation or
        // panic can only lose timer authority fail-stop.
        let mut timer = self
            .modem_lp_timer
            .take()
            .ok_or(BluetoothModemLpTimerOwnerError::OwnerSeparated)?;
        timer.prepare_registers();
        Ok(ModemLpTimerRegistersPreparedOwner { timer })
    }
}

/// Why task context cannot perform a modem LP-timer transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerOwnerError {
    /// The disjoint timer partition has already moved to source-127 storage.
    OwnerSeparated,
}

/// Opaque HAL owner after the source-127 controller-register prefix.
///
/// It deliberately exposes no task/cold escape or rollback:
///
/// ```compile_fail
/// use oer_esp32s31_hal::bluetooth::ModemLpTimerRegistersPreparedOwner;
///
/// fn bypass_remaining_init(prepared: ModemLpTimerRegistersPreparedOwner) {
///     let _task = prepared.into_task();
/// }
/// ```
#[must_use = "the prepared modem LP-timer owner must continue through route setup"]
pub struct ModemLpTimerRegistersPreparedOwner {
    timer: BluetoothModemLpTimerRegisters,
}

impl ModemLpTimerRegistersPreparedOwner {
    /// Complete the low-power hardware component with the retained task owner.
    ///
    /// # Safety
    ///
    /// The caller must retain the matching initialized controller software
    /// environment and must invoke this after the timer-register prefix but
    /// before source 127 can reach the CPU.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller must retain the matching controller software and inactive source-127 route"
    )]
    pub unsafe fn initialize_low_power_hardware(
        self,
        task: &mut TaskOwner,
    ) -> ModemLpTimerLowPowerHardwareInitializedOwner {
        task.reunitable = false;
        let mut timer = self.timer;
        let runtime_control = timer.initialize_low_power_hardware(&task.registers);
        ModemLpTimerLowPowerHardwareInitializedOwner {
            timer,
            runtime_control,
        }
    }
}

/// Opaque HAL owner after the complete low-power hardware component.
#[must_use = "the initialized modem LP-timer owner must continue through route setup"]
pub struct ModemLpTimerLowPowerHardwareInitializedOwner {
    timer: BluetoothModemLpTimerRegisters,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

impl ModemLpTimerLowPowerHardwareInitializedOwner {
    /// Return the positional runtime-control branch observed during initialization.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Start the runtime timer exactly once for this affine hardware epoch.
    ///
    /// Consuming this owner proves the command cannot be repeated through
    /// the same timer epoch.
    pub fn start_runtime_timer(self) -> ModemLpTimerCounterStartedOwner {
        let mut timer = self.timer;
        timer.start_runtime_counter();
        ModemLpTimerCounterStartedOwner {
            timer,
            runtime_control: self.runtime_control,
        }
    }
}

/// Opaque HAL owner after the one-shot BTDM runtime-timer start command.
#[must_use = "the started modem LP timer must continue through route setup"]
pub struct ModemLpTimerCounterStartedOwner {
    timer: BluetoothModemLpTimerRegisters,
    runtime_control: BluetoothLowPowerRuntimeControlObservation,
}

impl ModemLpTimerCounterStartedOwner {
    /// Return the low-power runtime-control branch retained across start.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.runtime_control
    }

    /// Transfer the unique started timer into stable source-127 ISR storage.
    ///
    /// This transition performs no MMIO and exposes no raw PAC owner. The
    /// platform must store the returned value before enabling the CPU route
    /// and recover it only after that route is disabled and no hard handler
    /// remains in flight.
    pub fn stage_for_interrupt(self) -> ModemLpTimerInterruptReadyOwner {
        ModemLpTimerInterruptReadyOwner { timer: self.timer }
    }
}

/// Opaque HAL owner staged for the source-127 hard handler.
#[must_use = "the modem LP-timer interrupt owner must remain in stable ISR storage"]
pub struct ModemLpTimerInterruptReadyOwner {
    timer: BluetoothModemLpTimerRegisters,
}

/// Result of one bounded source-127 hard-handler register step.
#[must_use = "retain the ready owner or complete the required software handler"]
pub enum ModemLpTimerInterruptStep {
    /// `STATUS_0038` was zero and the owner is ready for a later IRQ entry.
    Spurious(ModemLpTimerInterruptReadyOwner),
    /// The reviewed path requires the common software timer handler.
    HandlerPending(ModemLpTimerHandlerPendingOwner),
}

/// Opaque HAL owner for the common modem timer handler's register phase.
///
/// No direct rearm or task-owner escape exists. The owner must execute the
/// bounded register step, which either rearms an idle handler or produces the
/// separate fail-closed software-pending state.
#[must_use = "the common modem LP-timer register phase remains pending"]
pub struct ModemLpTimerHandlerPendingOwner {
    timer: BluetoothModemLpTimerRegisters,
    observation: BluetoothModemLpTimerInterruptObservation,
}

impl ModemLpTimerHandlerPendingOwner {
    /// Return the exact positional path that selected handler dispatch.
    pub const fn observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.observation
    }

    /// Execute the bounded register-acknowledgement phase of the common timer
    /// handler without invoking software or an RTOS service.
    pub fn step_registers(self) -> ModemLpTimerHandlerRegisterStep {
        let mut timer = self.timer;
        match timer.acknowledge_handler_registers() {
            None => {
                ModemLpTimerHandlerRegisterStep::Rearmed(ModemLpTimerInterruptReadyOwner { timer })
            }
            Some(register_observation) => {
                ModemLpTimerHandlerRegisterStep::SoftwarePending(ModemLpTimerSoftwarePendingOwner {
                    timer,
                    interrupt_observation: self.observation,
                    register_observation,
                })
            }
        }
    }
}

/// Result of the common handler's bounded register-acknowledgement phase.
#[must_use = "retain the ready owner or complete the required software work"]
pub enum ModemLpTimerHandlerRegisterStep {
    /// No software work was requested and the interrupt owner is ready again.
    Rearmed(ModemLpTimerInterruptReadyOwner),
    /// At least one acknowledged state byte requires software work.
    SoftwarePending(ModemLpTimerSoftwarePendingOwner),
}

/// Opaque HAL owner retained while source-127 software work is pending.
///
/// This owner has no raw PAC escape and cannot be rearmed until the no-RTOS
/// timer runtime supplies the missing software transition and final register
/// read.
#[must_use = "software timer work and the final hardware read remain pending"]
pub struct ModemLpTimerSoftwarePendingOwner {
    timer: BluetoothModemLpTimerRegisters,
    interrupt_observation: BluetoothModemLpTimerInterruptObservation,
    register_observation: BluetoothModemLpTimerHandlerRegisterObservation,
}

impl ModemLpTimerSoftwarePendingOwner {
    /// Return the initial source-127 classifier path.
    pub const fn interrupt_observation(&self) -> BluetoothModemLpTimerInterruptObservation {
        self.interrupt_observation
    }

    /// Return the positional state bytes requiring software consequences.
    pub const fn register_observation(&self) -> BluetoothModemLpTimerHandlerRegisterObservation {
        self.register_observation
    }

    /// Sample one finite positional LP-timer instant and acknowledge a newly
    /// observed rollover without polling.
    pub fn sample_counter(
        &mut self,
        epoch: &mut BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCounterObservation {
        self.timer.sample_counter(epoch)
    }

    /// Disable the currently programmed positional compare.
    pub fn disable_compare(&mut self) {
        self.timer.disable_compare();
    }

    /// Program one positional deadline and return the exact hardware branch.
    pub fn program_compare(
        &mut self,
        deadline: BluetoothModemLpTimerInstant,
        epoch: BluetoothModemLpTimerEpoch,
    ) -> BluetoothModemLpTimerCompareDisposition {
        self.timer.program_compare(deadline, epoch)
    }

    /// Perform the final fresh handler read and return the ISR-ready owner.
    ///
    /// This seam is hidden because the controller timer state machine must call
    /// it only after every software consequence represented by this owner has
    /// completed.
    #[doc(hidden)]
    pub fn complete_software(self) -> ModemLpTimerInterruptReadyOwner {
        let mut timer = self.timer;
        timer.sample_final_state();
        ModemLpTimerInterruptReadyOwner { timer }
    }
}

impl ModemLpTimerInterruptReadyOwner {
    /// Perform one finite source-127 register classification.
    ///
    /// The unique ISR-staged owner is the authority for exactly one register
    /// pass. The method never waits, loops, allocates or invokes an RTOS
    /// service.
    pub fn step(self) -> ModemLpTimerInterruptStep {
        let mut timer = self.timer;
        match timer.classify_interrupt() {
            None => ModemLpTimerInterruptStep::Spurious(ModemLpTimerInterruptReadyOwner { timer }),
            Some(observation) => {
                ModemLpTimerInterruptStep::HandlerPending(ModemLpTimerHandlerPendingOwner {
                    timer,
                    observation,
                })
            }
        }
    }
}

/// Why the opaque HAL task owner cannot return to cold ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOwnerReuniteError {
    /// A mutable controller or shared-PHY capability was issued and complete
    /// hardware rollback has not returned it to the cold baseline.
    HardwareLifecycleNotRestored,
    /// Interrupt output setup or routing touched hardware and the complete
    /// interrupt-bank baseline has not been restored.
    InterruptLifecycleNotRestored,
    /// A controller-time request still belongs to the task-side worker.
    ControllerTimeLatchInFlight,
    /// Source 127 still retains the disjoint modem LP-timer owner.
    ModemLpTimerOwnerSeparated,
}

/// Failed task/IRQ reunion retaining both opaque HAL owners unchanged.
#[must_use = "failed Bluetooth reunion still owns both HAL partitions"]
pub struct TaskOwnerReuniteFailure {
    task: TaskOwner,
    interrupts: InterruptSetupOwner,
    error: TaskOwnerReuniteError,
}

impl TaskOwnerReuniteFailure {
    /// Return the finite reunion failure reason.
    pub const fn error(&self) -> TaskOwnerReuniteError {
        self.error
    }

    /// Recover both retained HAL owners and the failure reason.
    pub fn into_parts(self) -> (TaskOwner, InterruptSetupOwner, TaskOwnerReuniteError) {
        (self.task, self.interrupts, self.error)
    }
}

impl core::fmt::Debug for TaskOwnerReuniteFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskOwnerReuniteFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Opaque inactive owner of the controller interrupt partition.
#[must_use = "the inactive Bluetooth interrupt owner must be staged or reunited"]
pub struct InterruptSetupOwner {
    registers: PacBluetoothInterruptSetup,
    reunitable: bool,
}

impl InterruptSetupOwner {
    /// Execute the reviewed baseline clear/enable/output preparation.
    ///
    /// The controller lifecycle owns completed HAL-init and quiescent dynamic
    /// sources. The returned state remains retained until stable ISR storage
    /// is ready.
    ///
    /// # Safety
    ///
    /// The caller must retain completed controller HAL-init, quiescent dynamic
    /// sources, and inactive primary/NRT CPU routes for this interrupt owner.
    #[allow(
        unsafe_code,
        reason = "the caller must retain completed controller init and inactive CPU routes"
    )]
    pub unsafe fn prepare_controller_output(self) -> InterruptOutputPreparedOwner {
        InterruptOutputPreparedOwner {
            registers: self.registers.prepare_controller_output(),
        }
    }
}

/// Controller IRQ output prepared but not yet transferred into stable ISR
/// storage or bound to a CPU route.
#[must_use = "the prepared Bluetooth output must be staged or released"]
pub struct InterruptOutputPreparedOwner {
    registers: oer_esp32s31_pac::BluetoothInterruptOutputPrepared,
}

impl InterruptOutputPreparedOwner {
    /// Transfer the register partition to the state required by shared ISR
    /// storage before either platform route is enabled.
    pub fn stage_for_cpu_routes(self) -> InterruptRegistersOwner {
        InterruptRegistersOwner {
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
    pub fn release_controller_output(self) -> InterruptSetupOwner {
        InterruptSetupOwner {
            registers: self.registers.release_controller_output(),
            reunitable: false,
        }
    }
}

/// Opaque interrupt-register owner staged for primary and NRT CPU routes.
#[must_use = "the staged Bluetooth interrupt owner must be deactivated"]
pub struct InterruptRegistersOwner {
    registers: BluetoothInterruptRegisters,
}

impl InterruptRegistersOwner {
    /// Prepare the dynamic interrupt groups required immediately before a
    /// scheduler run publication.
    pub fn prepare_scheduler_run_interrupts(&mut self) -> BluetoothSchedulerRunInterruptsPrepared {
        self.registers.prepare_scheduler_run_interrupts()
    }

    /// Capture and acknowledge one complete opaque NRT source-133 epoch.
    ///
    /// This preserves the PAC sample/sample/acknowledge/acknowledge order. It
    /// deliberately performs no feature dispatch and publishes no task wake.
    pub fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged {
        self.registers.capture_nrt_and_acknowledge()
    }

    /// Capture, acknowledge and retain one complete primary source-124 epoch.
    pub fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.registers.capture_primary_and_acknowledge()
    }

    /// Capture the first scheduler-state observation used only by the
    /// bank-one source-3 reference gate.
    pub fn capture_scheduler_reference_gate(
        &mut self,
    ) -> BluetoothSchedulerReferenceGateObservation {
        self.registers.capture_scheduler_reference_gate()
    }

    /// Clear the scheduler reference selected by the source-124 gate.
    ///
    /// The returned affine token proves the ordered PAC write and device fence.
    /// The open scheduler's software consistency is structural and does not
    /// reproduce the vendor's intrusive-list callback.
    pub fn clear_scheduler_reference(&mut self) -> BluetoothSchedulerReferenceCleared {
        self.registers.clear_scheduler_reference()
    }

    /// Capture the later, independent scheduler-state observation used to
    /// construct deferred work.
    pub fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.registers.capture_scheduler_work()
    }

    /// Capture the interrupt-owned scheduler BUSY field for one lock/modify
    /// decision without borrowing task-side controller registers.
    pub fn capture_scheduler_lock_modify_interrupt(
        &mut self,
    ) -> BluetoothSchedulerLockModifyInterruptObservation {
        self.registers.capture_scheduler_lock_modify_interrupt()
    }

    /// Return the register partition to output-prepared ownership after both
    /// CPU routes have been disabled and shared ISR access has ended.
    ///
    /// The returned state requires the terminal idle/head/fault preflight
    /// before it can release Controller output.
    pub fn deactivate(self) -> InterruptOutputAfterRoutesOwner {
        InterruptOutputAfterRoutesOwner {
            _registers: self.registers.deactivate(),
        }
    }
}

/// Controller interrupt bank recovered from stable ISR storage after both CPU
/// routes were disabled.
///
/// Checked output release requires the matching task registers and fresh
/// scheduler/head/fault observations. No direct setup or cold conversion exists.
#[must_use = "post-route interrupt ownership awaits dynamic-source quiescence"]
pub struct InterruptOutputAfterRoutesOwner {
    _registers: oer_esp32s31_pac::BluetoothInterruptOutputPrepared,
}

impl InterruptOutputAfterRoutesOwner {
    /// Observe quiescence while retaining the prepared, unrouted output bank.
    pub fn validate_idle_controller(
        &self,
        task: &mut TaskOwner,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        if task.time_latch.in_flight() {
            return Err(BluetoothControllerOutputReleaseError::ControllerTimePending);
        }
        self._registers
            .validate_idle_controller(&mut task.registers)
    }

    /// Revalidate quiescence after maintenance and return the same prepared bank.
    /// This does not enable CPU routes or publish either ISR owner.
    pub fn try_reactivate_idle_controller_output(
        self,
        task: &mut TaskOwner,
    ) -> Result<InterruptRegistersOwner, (BluetoothControllerOutputReleaseError, Self)> {
        if let Err(error) = self.validate_idle_controller(task) {
            return Err((error, self));
        }
        Ok(InterruptRegistersOwner {
            registers: self._registers.stage_for_cpu_routes(),
        })
    }

    /// Mask and acknowledge the idle scheduler's dynamic sources, then release
    /// Controller output. A busy scheduler, published head, pending time latch
    /// or primary fault retains both owners. Rejection can leave RUN disabled;
    /// it never authorizes reactivation, memory reclamation or cold reunion.
    pub fn try_release_idle_controller_output(
        self,
        task: &mut TaskOwner,
    ) -> Result<InterruptOutputReleasedOwner, (BluetoothControllerOutputReleaseError, Self)> {
        if task.time_latch.in_flight() {
            return Err((
                BluetoothControllerOutputReleaseError::ControllerTimePending,
                self,
            ));
        }
        match self
            ._registers
            .try_release_idle_controller_output(&mut task.registers)
        {
            Ok(registers) => Ok(InterruptOutputReleasedOwner { registers }),
            Err((error, registers)) => Err((
                error,
                Self {
                    _registers: registers,
                },
            )),
        }
    }
}

/// Output released after the post-route, idle-scheduler hardware preflight.
/// No active IRQ or setup conversion is exposed; retain it through RF close.
#[must_use = "released output must remain paired with its stopped Controller"]
pub struct InterruptOutputReleasedOwner {
    registers: PacBluetoothInterruptSetup,
}

/// Exclusive finite borrow of the Bluetooth controller task-side registers.
///
/// This type deliberately exposes neither `Deref`, a raw PAC accessor nor a
/// constructor. New operations belong here only after their PAC transaction
/// and lifecycle prerequisites are independently bounded.
pub struct ControllerHal<'registers> {
    registers: &'registers mut PacBluetoothTaskRegisters,
    time_latch: &'registers mut ControllerTimeLatch,
}

/// Public Bluetooth Controller identity in canonical display order.
///
/// Keeping the canonical representation in this type makes the sole reversal
/// into the Controller's least-significant-octet-first order an internal HAL
/// concern. It cannot be confused with a random address already decoded from
/// an HCI command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerPublicAddress {
    canonical_octets: [u8; 6],
}

impl ControllerPublicAddress {
    /// Construct a public identity from canonical EUI-48 display order.
    pub const fn from_canonical_bytes(canonical_octets: [u8; 6]) -> Self {
        Self { canonical_octets }
    }

    /// Return the canonical EUI-48 display order unchanged.
    pub const fn canonical_bytes(self) -> [u8; 6] {
        self.canonical_octets
    }

    const fn controller_wire_octets(self) -> [u8; 6] {
        let [octet_0, octet_1, octet_2, octet_3, octet_4, octet_5] = self.canonical_octets;
        [octet_5, octet_4, octet_3, octet_2, octet_1, octet_0]
    }
}

/// Random Bluetooth Controller identity in HCI/Link-Layer wire order.
///
/// `LE Set Random Address` already carries the least-significant address octet
/// first. This distinct type prevents that representation from being reversed
/// a second time before publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerRandomAddress {
    hci_wire_octets: [u8; 6],
}

impl ControllerRandomAddress {
    /// Construct a random identity from an HCI `BD_ADDR` payload.
    pub const fn from_hci_wire_bytes(hci_wire_octets: [u8; 6]) -> Self {
        Self { hci_wire_octets }
    }

    /// Return the HCI/Link-Layer order unchanged.
    pub const fn hci_wire_bytes(self) -> [u8; 6] {
        self.hci_wire_octets
    }

    const fn controller_wire_octets(self) -> [u8; 6] {
        self.hci_wire_octets
    }
}

/// One initialized receive-memory list published to the controller.
///
/// The token is intentionally affine. It records the positional list and its
/// validated head without granting access to either memory or MMIO. The
/// memory owner must retain the complete pinned graph until a later verified
/// retirement transaction consumes this publication.
#[must_use = "the published receive list remains owned by the controller"]
pub struct RxMemoryListPublished {
    selector: BluetoothMemoryListSelector,
    head: BluetoothControllerSramAddress,
}

/// HAL ownership of the controller-global disabled-CTE publication.
///
/// The restricted PAC proof remains private, so upper Controller layers can
/// retain the hardware epoch without depending on a register-level type.
#[must_use = "the disabled-CTE publication must retain its pinned workspace"]
pub struct DirectionFindingDisabledBaselineOwner {
    _prepared: BluetoothDirectionFindingDisabledBaselinePrepared,
}

impl RxMemoryListPublished {
    /// Construct a semantic publication proof for host ownership validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_parts_for_validation(
        selector: BluetoothMemoryListSelector,
        head: BluetoothControllerSramAddress,
    ) -> Self {
        Self { selector, head }
    }

    /// Return the positional hardware-list selector chosen by the memory
    /// layer.
    pub const fn selector(&self) -> BluetoothMemoryListSelector {
        self.selector
    }

    /// Return the validated first-node address without granting dereference
    /// access.
    pub const fn head(&self) -> BluetoothControllerSramAddress {
        self.head
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RxPacketControl {
    ControllerDefault,
    SoftwareConnection,
}

trait RxMemoryListInitialPublication {
    fn prepare_packet_control(&mut self, policy: RxPacketControl);
    fn publish_current_head(&mut self);
    fn clear_next_head(&mut self);
    fn reset_initial_control(&mut self);
}

fn execute_rx_memory_list_initial_publication(
    transaction: &mut impl RxMemoryListInitialPublication,
    policy: RxPacketControl,
) {
    transaction.prepare_packet_control(policy);
    transaction.publish_current_head();
    transaction.clear_next_head();
    transaction.reset_initial_control();
}

struct PacBluetoothRxMemoryListInitialPublication<'registers> {
    registers: &'registers mut PacBluetoothTaskRegisters,
    selector: BluetoothMemoryListSelector,
    head: BluetoothControllerSramAddress,
}

impl RxMemoryListInitialPublication for PacBluetoothRxMemoryListInitialPublication<'_> {
    fn prepare_packet_control(&mut self, policy: RxPacketControl) {
        match policy {
            RxPacketControl::ControllerDefault => {
                self.registers.restore_default_connection_packet_control();
            }
            RxPacketControl::SoftwareConnection => {
                self.registers.prepare_software_connection_packet_control();
            }
        }
    }

    #[allow(
        unsafe_code,
        reason = "the transaction retains the serialized RX-list lifecycle"
    )]
    fn reset_initial_control(&mut self) {
        // SAFETY: the enclosing operation retains the same pinned graph and
        // exclusive list lifecycle through pointer publication and this reset.
        unsafe {
            self.registers
                .reset_memory_list_initial_control(self.selector)
        };
    }

    #[allow(
        unsafe_code,
        reason = "the enclosing HAL operation retains list lifetime and controller-lifecycle prerequisites"
    )]
    fn publish_current_head(&mut self) {
        // SAFETY: this transaction exists only inside the two unsafe RX-list
        // initial-publication operations, whose callers retain the pinned
        // list at `head` and the serialized controller lifecycle.
        unsafe {
            self.registers.program_memory_list_pointer(
                self.selector,
                BluetoothMemoryListSlot::CurrentRx,
                BluetoothMemoryListPointerImage::Address(self.head),
            );
        }
    }

    #[allow(
        unsafe_code,
        reason = "the enclosing HAL operation retains list lifetime and controller-lifecycle prerequisites"
    )]
    fn clear_next_head(&mut self) {
        // SAFETY: as for `publish_current_head`; a zero next pointer
        // references no storage.
        unsafe {
            self.registers.program_memory_list_pointer(
                self.selector,
                BluetoothMemoryListSlot::NextRx,
                BluetoothMemoryListPointerImage::Zero,
            );
        }
    }
}

impl ControllerHal<'_> {
    /// Publish the public identity through the Controller's fixed public slot.
    ///
    /// The owning lifecycle must retain the powered BLE Controller epoch and
    /// prove that no active advertising, scanning or connection operation can
    /// consume the address pair during replacement. Cold start must invoke
    /// this after BLE PHY initialization and before interrupt publication or
    /// radio work.
    #[doc(hidden)]
    pub fn program_public_device_address(&mut self, address: ControllerPublicAddress) {
        self.registers
            .program_bluetooth_public_device_address(address.controller_wire_octets());
    }

    /// Publish the selected random identity through its fixed Controller slot.
    ///
    /// The owning lifecycle must retain the powered BLE Controller epoch and
    /// prove that no active advertising, scanning or connection operation can
    /// consume the address pair during replacement. A role-start path must
    /// perform this while idle and before publishing its scheduler head or
    /// `RUN`.
    #[doc(hidden)]
    pub fn program_random_device_address(&mut self, address: ControllerRandomAddress) {
        self.registers
            .program_bluetooth_random_device_address(address.controller_wire_octets());
    }

    /// Publish the controller-global CTE-disabled descriptor baseline.
    ///
    /// # Safety
    ///
    /// The caller must retain the powered Controller epoch and the matching
    /// initialized pinned descriptor until a reviewed retirement transition.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains powered-controller and descriptor-lifetime prerequisites"
    )]
    pub unsafe fn prepare_direction_finding_disabled_baseline(
        &mut self,
        descriptor: BluetoothControllerSramAddress,
    ) -> DirectionFindingDisabledBaselineOwner {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        let prepared = unsafe {
            self.registers
                .prepare_direction_finding_disabled_baseline(descriptor)
        };
        DirectionFindingDisabledBaselineOwner {
            _prepared: prepared,
        }
    }

    /// Publish one initialized receive-memory list in the reviewed cold order.
    ///
    /// The memory layer owns the semantic mapping from a controller role to
    /// the positional `selector`. HAL publishes the initialized current head
    /// first and only then clears the matching next head. Neither positional
    /// slot nor its register representation crosses this boundary.
    /// The controller-default opcode policy is restored before either pointer;
    /// software-owned connections use their separate publication transaction.
    ///
    /// # Safety
    ///
    /// The caller must own the selected powered-controller lifecycle epoch,
    /// must have no active RUN, retain the complete initialized pinned memory graph, and
    /// serialize all task and interrupt access to this list until a later
    /// verified retirement transaction consumes the returned token.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains pinned-list lifetime and controller-lifecycle prerequisites"
    )]
    pub unsafe fn publish_rx_memory_list_initial_head(
        &mut self,
        selector: BluetoothMemoryListSelector,
        head: BluetoothControllerSramAddress,
    ) -> RxMemoryListPublished {
        let mut transaction = PacBluetoothRxMemoryListInitialPublication {
            registers: self.registers,
            selector,
            head,
        };
        execute_rx_memory_list_initial_publication(
            &mut transaction,
            RxPacketControl::ControllerDefault,
        );
        RxMemoryListPublished { selector, head }
    }

    /// Publish RX for a connection whose control PDUs are owned by software.
    ///
    /// Configure opcode inspection before publishing the RX head. Software CCM
    /// ciphertext must reach the authenticating LL decoder even when its first
    /// byte matches a hardware abort opcode. Repeating this transaction also
    /// restores the policy after PHY maintenance reinitializes BLE registers.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned connection graph and the
    /// exclusive powered task epoch, with no active RUN. It must serialize RX
    /// list access until the returned publication is retired.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the connection owner retains the powered graph and idle hardware"
    )]
    pub unsafe fn publish_software_connection_rx_memory_list_initial_head(
        &mut self,
        selector: BluetoothMemoryListSelector,
        head: BluetoothControllerSramAddress,
    ) -> RxMemoryListPublished {
        let mut transaction = PacBluetoothRxMemoryListInitialPublication {
            registers: self.registers,
            selector,
            head,
        };
        execute_rx_memory_list_initial_publication(
            &mut transaction,
            RxPacketControl::SoftwareConnection,
        );
        RxMemoryListPublished { selector, head }
    }

    /// Publish one complete reviewed scanner command transaction.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned scanner graph, its
    /// matching RX-list publication and an exclusive powered controller epoch.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains scanner graph lifetime and controller-lifecycle prerequisites"
    )]
    pub unsafe fn publish_scan_start(&mut self) -> BluetoothScanStartPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.publish_scan_start() }
    }

    /// Remove every published scheduler hardware-list head.
    ///
    /// This is only the reviewed controller-initialization prefix. It does not
    /// establish scheduler, Link Layer or HCI readiness. The borrow proves
    /// exclusive register access, not powered lifecycle state; the caller must
    /// retain the independently established clock/reset prerequisite.
    pub fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        self.registers.clear_scheduler_hardware_list_heads()
    }

    /// Perform one fresh fenced post-completion head observation while
    /// retaining the exact affine RUN provenance on every result path.
    pub fn observe_scheduler_hardware_list_head_retirement(
        &mut self,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerHardwareListHeadRetirementObservation {
        self.registers
            .observe_scheduler_hardware_list_head_retirement(run)
    }

    /// Order prior descriptor writes and publish one scheduler hardware-list
    /// head through the restricted PAC.
    ///
    /// # Safety
    ///
    /// The caller must retain the scheduler insertion epoch, completed
    /// descriptor initialization, graph lifetime and task/interrupt
    /// serialization required by the underlying PAC transaction. The PAC
    /// orders those prior SRAM writes before the MMIO head update.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler-epoch prerequisites"
    )]
    pub unsafe fn publish_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe {
            self.registers
                .publish_scheduler_hardware_list_head(index, head)
        }
    }

    /// Clear START in the insertion command selected by the Controller.
    ///
    /// # Safety
    ///
    /// The caller must retain the matching insertion-begin result and must
    /// serialize the command RMW with every producer and interrupt owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the insertion result and command serialization"
    )]
    pub unsafe fn clear_scheduler_insertion_command_start(
        &mut self,
        command: BluetoothSchedulerInsertionCommand,
    ) -> BluetoothSchedulerInsertionCommandStartCleared {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe {
            self.registers
                .clear_scheduler_insertion_command_start(command)
        }
    }

    /// Publish the insertion-begin execution-lock command through the
    /// restricted PAC.
    ///
    /// # Safety
    ///
    /// The request must identify the exact merge-selected initialized item.
    /// The caller must retain that pinned item and exclusive list ownership
    /// through the complete insertion transaction.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains merge-selected item lifetime and list serialization"
    )]
    pub unsafe fn publish_scheduler_execution_lock(
        &mut self,
        request: BluetoothSchedulerExecutionLockRequest,
    ) -> BluetoothSchedulerExecutionLockPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.publish_scheduler_execution_lock(request) }
    }

    /// Perform one finite typed command-zero observation in its reviewed
    /// short-circuit order.
    pub fn observe_scheduler_execution_lock(
        &mut self,
        scheduler: BluetoothSchedulerWorkObservation,
    ) -> BluetoothSchedulerExecutionLockDisposition {
        self.registers.observe_scheduler_execution_lock(scheduler)
    }

    /// Publish the insertion-begin execution-modify command through the
    /// restricted PAC.
    ///
    /// # Safety
    ///
    /// The caller must retain the insertion reconciliation epoch and
    /// exclusive ownership of `index` through insertion-end.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains insertion reconciliation and list serialization"
    )]
    pub unsafe fn publish_scheduler_execution_modify(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerExecutionModifyPublished {
        // SAFETY: forwarded unchanged from this function's `# Safety` contract,
        // which states the PAC transaction's prerequisites.
        unsafe { self.registers.publish_scheduler_execution_modify(index) }
    }

    /// Perform one finite typed command-one observation in its reviewed
    /// short-circuit order.
    pub fn observe_scheduler_execution_modify(
        &mut self,
        scheduler: BluetoothSchedulerWorkObservation,
    ) -> BluetoothSchedulerExecutionModifyDisposition {
        self.registers.observe_scheduler_execution_modify(scheduler)
    }

    /// Publish the synchronous BTMAC scheduler event through the restricted
    /// PAC after the matching head and dynamic interrupts are prepared.
    #[doc(hidden)]
    pub fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        self.registers.publish_scheduler_run_event(head, interrupts)
    }

    /// Publish the final scheduler hardware RUN command through the
    /// restricted PAC. The consumed event retains the complete run prologue.
    #[doc(hidden)]
    pub fn publish_scheduler_hardware_run_command(
        &mut self,
        event: BluetoothSchedulerRunEventPublished,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        self.registers.publish_scheduler_hardware_run_command(event)
    }

    /// Capture task-owned START and RESULT fields for one scheduler
    /// lock/modify decision.
    pub fn capture_scheduler_lock_modify_task(
        &mut self,
    ) -> BluetoothSchedulerLockModifyTaskObservation {
        self.registers.capture_scheduler_lock_modify_task()
    }

    /// Finish one finite task-owned software-list removal observation after a
    /// fresh interrupt-side idle sample.
    pub fn finish_scheduler_software_list_removal(
        &mut self,
        idle: BluetoothSchedulerSoftwareListRemovalIdle,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> BluetoothSchedulerSoftwareListRemovalJoin {
        self.registers
            .finish_scheduler_software_list_removal(idle, head)
    }

    /// Advance the common stop preamble/request/idle sequence under interrupt
    /// serialization. Pending retains the request; the caller owns its deadline.
    pub fn step_scheduler_stop(
        &mut self,
        interrupts: &mut InterruptRegistersOwner,
        stop: BluetoothSchedulerStop,
    ) -> BluetoothSchedulerStopStep {
        scheduler_stop::step_hardware(self.registers, &mut interrupts.registers, stop)
    }

    /// Retire the exact stopped RUN head without clearing a foreign item.
    pub fn retire_stopped_scheduler_head(
        &mut self,
        stopped: BluetoothSchedulerStopped,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerStoppedHeadRetirement {
        self.registers.retire_stopped_scheduler_head(stopped, run)
    }

    /// Perform one finite direct recheck of the complete post-unlink return
    /// predicate through both disjoint register owners.
    ///
    /// The PAC preserves the reviewed fresh-read order and short-circuiting:
    /// scheduler BUSY, command-zero status 26, then command-one status 18.
    /// `Pending` retains the affine empty-head proof for a later authorized
    /// recheck; `Ready` consumes it into the removal-complete proof.
    pub fn recheck_scheduler_software_list_removal(
        &mut self,
        interrupts: &mut InterruptRegistersOwner,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> BluetoothSchedulerSoftwareListRemovalJoin {
        self.registers
            .recheck_scheduler_software_list_removal(&mut interrupts.registers, head)
    }

    /// Transfer one fresh hardware finished-list observation to its reviewed
    /// report register and return the typed sixteen-list projection.
    ///
    /// This finite task-side operation performs no item lookup or callback.
    /// The Controller must retain descriptor ownership until a separately
    /// proven completion-visibility fence permits CPU access.
    pub fn transfer_scheduler_finished_lists(
        &mut self,
    ) -> BluetoothSchedulerFinishedListObservation {
        self.registers.transfer_scheduler_finished_lists()
    }

    /// Execute the finite scheduler lock/modify publication transaction.
    ///
    /// The Bluetooth controller state machine keeps this method hidden behind
    /// its affine publication token. It performs two fresh operational-word
    /// RMW edges, publishes the typed request and returns only after a device
    /// fence; it never polls or blocks an executor.
    ///
    /// # Safety
    ///
    /// The caller must retain the initialized pinned scheduler item named by
    /// `request` and exclusive scheduler task/interrupt serialization until
    /// verified Controller ownership return.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler serialization"
    )]
    pub unsafe fn publish_scheduler_lock_modify(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        // SAFETY: this method forwards the identical descriptor-lifetime and
        // serialization obligations to the restricted PAC transaction.
        unsafe { self.registers.publish_scheduler_lock_modify(request) }
    }

    /// Publish one controller-time latch request and return immediately.
    ///
    /// The powered lifecycle must first establish a reset/quiescent timer
    /// domain. Entering the Bluetooth ownership route alone performs no reset
    /// and deliberately does not treat an arbitrary hardware bit image as a
    /// fresh request owned by this driver.
    ///
    /// The unique task owner remembers the request across HAL borrows. If an
    /// async operation is cancelled before `Ready`, another begin fails closed
    /// and the durable task owner must drain that same request with
    /// [`Self::step_controller_time_latch`] before admitting new work.
    pub fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        controller_time::begin(self.time_latch, self.registers)
    }

    /// Perform exactly one observation of the controller-time latch.
    ///
    /// `Waiting` means hardware still owns the request and the caller should
    /// yield until an interrupt or bounded timer event. This method never
    /// loops, registers a waker, allocates or depends on an RTOS.
    pub fn step_controller_time_latch(
        &mut self,
    ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
        controller_time::step(self.time_latch, self.registers)
    }

    /// Whether this task owner retains an unfinished latch request.
    ///
    /// This is a diagnostic view for the durable controller task owner. A
    /// cancelled logical operation must be drained before a fresh request is
    /// begun; its sample must not be relabelled as that fresh request.
    pub fn controller_time_latch_in_flight(&self) -> bool {
        self.time_latch.in_flight()
    }
}

mod sealed {

    use crate::bluetooth::TaskOwner;

    pub trait ControllerHalBorrow {
        fn bluetooth_task_owner_mut(&mut self) -> &mut TaskOwner;
    }

    impl ControllerHalBorrow for TaskOwner {
        fn bluetooth_task_owner_mut(&mut self) -> &mut TaskOwner {
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
/// use oer_esp32s31_hal::bluetooth::ControllerHalBorrow;
///
/// fn duplicate(owner: &mut impl ControllerHalBorrow) {
///     let first = owner.borrow_bluetooth_controller();
///     let second = owner.borrow_bluetooth_controller();
///     let _ = (first, second);
/// }
/// ```
#[doc(hidden)]
pub trait ControllerHalBorrow: sealed::ControllerHalBorrow {
    /// Borrow the controller registers without exposing their PAC owner.
    fn borrow_bluetooth_controller(&mut self) -> ControllerHal<'_> {
        let owner = sealed::ControllerHalBorrow::bluetooth_task_owner_mut(self);
        owner.reunitable = false;
        ControllerHal {
            registers: &mut owner.registers,
            time_latch: &mut owner.time_latch,
        }
    }
}

impl ControllerHalBorrow for TaskOwner {}

#[cfg(test)]
mod tests;
