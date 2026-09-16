//! Hardware-only modem low-power initialization and runtime ownership.

use crate::{
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
    },
    scheduler::{SchedulerInitialized, SchedulerSoftwareConfig},
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerOwnerError,
    ModemLpTimerLowPowerHardwareInitializedOwner,
};

use oer_esp32s31_pac::BluetoothControllerTimeScale;

/// Powered Controller ownership after the complete modem low-power hardware
/// component and before source 127 is installed.
///
/// The disjoint timer owner stays inside this state. It is not yet in stable
/// ISR storage, no CPU route is enabled, and this state therefore exposes no
/// live timer interrupt or operational Link-Layer capability.
#[must_use = "the initialized low-power hardware retains every powered Bluetooth owner"]
pub struct ControllerLowPowerHardwareInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    scheduler: SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    timer_hardware: Option<ModemLpTimerLowPowerHardwareInitializedOwner>,
}

/// Lossless failure to initialize the disjoint modem low-power timer owner.
#[must_use = "the powered scheduler remains owned after a rejected transition"]
pub struct ControllerLowPowerHardwareInitializationFailure<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    scheduler: SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    error: BluetoothModemLpTimerOwnerError,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerLowPowerHardwareInitializationFailure<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Exact lower ownership error.
    pub const fn error(&self) -> BluetoothModemLpTimerOwnerError {
        self.error
    }

    /// Recover the complete powered Controller owner.
    pub fn into_scheduler(
        self,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        self.scheduler
    }
}

/// Exclusive task HAL lease and borrowed software endpoints from one epoch.
///
/// Named fields keep interrupt, task and modem-timer roles explicit. HCI is not
/// part of this hardware-only split and is joined only after stable interrupt
/// publication.
#[must_use = "all runtime endpoints belong to one powered Controller epoch"]
pub struct ControllerRuntimeEndpoints<
    'runtime,
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    /// Interrupt-side scheduler publications.
    pub interrupt: ControllerInterruptRuntime<'runtime>,
    /// Exclusive task-side HAL lease and borrowed scheduler workers.
    pub task: ControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    /// Unique mutable source-127 queue and epoch runtime.
    pub modem_timer: ControllerModemTimerRuntime<'runtime, MODEM_TIMER_CAPACITY>,
    /// Exclusive powered platform reservation; independent of command states.
    pub platform: crate::resources::platform_retirement::ControllerPlatformLease<'runtime, P>,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc test seam returns the complete affine powered owner"
    )]
    fn try_initialize_low_power_hardware_with<TimerHardware, Error>(
        mut self,
        initialize: impl FnOnce(&mut crate::resources::TaskResources) -> Result<TimerHardware, Error>,
    ) -> Result<(Self, TimerHardware), (Self, Error)> {
        match initialize(self.task_mut()) {
            Ok(timer_hardware) => Ok((self, timer_hardware)),
            Err(error) => Err((self, error)),
        }
    }

    /// Execute the source-127 register prefix and complete modem low-power
    /// hardware initialization while the CPU route remains inactive.
    ///
    /// The safe Controller typestate discharges the lower HAL prerequisites:
    /// clocks, scheduler software and both disjoint register owners belong
    /// to this exact powered epoch, while the only route installer remains
    /// private and cannot have run yet.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the consuming Controller state proves the lower HAL prerequisites"
    )]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the complete affine powered owner"
    )]
    pub fn initialize_modem_lp_timer_hardware(
        self,
    ) -> Result<
        ControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        ControllerLowPowerHardwareInitializationFailure<
            P,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
    > {
        match self.try_initialize_low_power_hardware_with(|task| {
            // SAFETY: this consuming state owns the powered scheduler
            // epoch and no public transition can have installed source 127.
            unsafe { task.initialize_modem_lp_timer_hardware() }
        }) {
            Ok((scheduler, timer_hardware)) => Ok(ControllerLowPowerHardwareInitialized {
                scheduler,
                timer_hardware: Some(timer_hardware),
            }),
            Err((scheduler, error)) => {
                Err(ControllerLowPowerHardwareInitializationFailure { scheduler, error })
            }
        }
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Number of scheduler modem-timer slots retained by this exact epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.scheduler.modem_timer_capacity()
    }

    /// Number of source-owned scheduler reservations retained by this epoch.
    pub const fn scheduler_capacity(&self) -> usize {
        self.scheduler.scheduler_capacity()
    }

    /// Scheduler time scale retained by this exact powered epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.scheduler.controller_time_scale()
    }

    /// Source-owned scheduler policy retained by this exact powered epoch.
    pub const fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        self.scheduler.scheduler_config()
    }

    /// Whether no scheduler software work entered this epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.scheduler.runtime_is_pristine()
    }

    /// Claim the task HAL slot once and borrow matching software endpoints.
    /// The timer-hardware owner remains retained. `None` means the task was
    /// already claimed; no second runtime is created.
    pub fn split_runtime(
        &mut self,
    ) -> Option<ControllerRuntimeEndpoints<'_, P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>> {
        let (interrupt, task, modem_timer, platform) = self.scheduler.split_runtime()?;
        Some(ControllerRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
            platform,
        })
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(
        &mut self,
    ) -> (&mut crate::resources::TaskResources, &mut P) {
        self.scheduler.common_phy_parts_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn task_mut(&mut self) -> &mut crate::resources::TaskResources {
        self.scheduler.task_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> crate::resources::InterruptBankOwner {
        self.scheduler.take_interrupt_owner()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_timer_hardware(&mut self) -> ModemLpTimerLowPowerHardwareInitializedOwner {
        self.timer_hardware.take().expect(
            "private Controller invariant retains the low-power timer owner until activation",
        )
    }

    /// Conditional runtime-control branch observed by the exact hardware
    /// component.
    pub const fn runtime_control_observation(&self) -> BluetoothLowPowerRuntimeControlObservation {
        self.timer_hardware
            .as_ref()
            .expect("public low-power state always retains its timer owner")
            .runtime_control_observation()
    }
}

#[cfg(test)]
mod tests;

#[cfg(target_arch = "riscv32")]
impl<P, const MT: usize, const SC: usize> ControllerLowPowerHardwareInitialized<P, MT, SC> {
    pub(crate) fn into_restart_parts(
        self,
    ) -> crate::scheduler::core::SchedulerRestartParts<P, MT, SC> {
        assert!(
            self.timer_hardware.is_none(),
            "timer partition already staged"
        );
        self.scheduler.into_restart_parts()
    }
}
