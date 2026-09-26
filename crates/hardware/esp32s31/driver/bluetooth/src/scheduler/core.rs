//! Fact-bounded scheduler initialization after the controller HAL component.

use crate::scheduler::SchedulerSoftwareConfig;

use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerTimeScale, BluetoothSchedulerHardwareListsCleared,
};

use crate::{
    controller_hal::ControllerHalInitialized,
    resources::{InterruptBankOwner, TaskResources, TeardownPendingPlatform},
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
        ControllerRuntimeResources,
    },
};

/// Why one private DTM controller-time phase could not complete.
///
/// Post-enable timing, recurring-RX current, admission and sequence samples are
/// acquired by the Controller and never cross the public DTM preparation
/// boundary. This finite error retains only the logical acquisition outcome;
/// the role-specific preparation failure continues to own every retry resource.
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerTimeAcquisitionError {
    /// Another request or abandoned request still owns the latch worker.
    Busy,
    /// The logical worker and lower sticky latch owner disagreed at begin.
    OwnershipCollision,
    /// The non-repeating request generation space was exhausted.
    GenerationExhausted,
    /// A recheck or cancellation named a different logical request.
    RequestMismatch,
    /// The lower sticky latch owner disappeared before completion.
    OwnershipLost,
    /// An earlier ownership disagreement stopped the latch worker.
    Faulted,
    /// The caller explicitly abandoned this phase before completion.
    Cancelled,
}

/// Hardware and source-owned software state after scheduler initialization.
///
/// This transition replaces the complete reviewed scheduler-init function:
/// all sixteen hardware list heads are removed, the scheduler policy is
/// retained without copying the vendor structure ABI, and one pristine static
/// Rust runtime replaces the vendor event object and generic broker nodes.
/// Typed event cells and workers make numeric broker source identifiers and an
/// intrusive callback list unnecessary.
///
/// Remaining hardware initialization and stable ISR publication are still
/// missing. This state therefore exposes no PHY, BTBB, IRQ, Controller or
/// Link-Layer readiness. HCI remains outside the
/// hardware boot chain until stable interrupt-owner publication completes.
/// Dropping this state is fail-stop because no verified rollback exists after
/// scheduler MMIO mutation.
#[must_use = "the initialized scheduler retains every powered Bluetooth owner"]
pub struct SchedulerInitialized<P, const MODEM_TIMER_CAPACITY: usize> {
    task: crate::resources::runtime_owner::RuntimeOwnerSlot<TaskResources>,
    _interrupts: Option<InterruptBankOwner>,
    _platform: crate::resources::runtime_owner::RuntimeOwnerSlot<TeardownPendingPlatform<P>>,
    time_scale: BluetoothControllerTimeScale,
    _standalone_dtm_profile: crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
    config: SchedulerSoftwareConfig,
    _hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared,
    runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
pub struct SchedulerRestartParts<P, const MT: usize> {
    pub task: TaskResources,
    pub platform: TeardownPendingPlatform<P>,
    pub time_scale: BluetoothControllerTimeScale,
    pub config: SchedulerSoftwareConfig,
    pub hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared,
    pub runtime: ControllerRuntimeResources<MT>,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MT: usize> SchedulerInitialized<P, MT> {
    pub(crate) fn into_restart_parts(self) -> SchedulerRestartParts<P, MT> {
        assert!(
            self._interrupts.is_none(),
            "interrupt partition already staged"
        );
        SchedulerRestartParts {
            task: self.task.into_unclaimed(),
            platform: self._platform.into_unclaimed(),
            time_scale: self.time_scale,
            config: self.config,
            hardware_lists_cleared: self._hardware_lists_cleared,
            runtime: self.runtime,
        }
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize> SchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
    pub(crate) fn task_mut(&mut self) -> &mut TaskResources {
        self.task
            .as_mut()
            .expect("pre-split scheduler owns its task")
    }

    #[cfg(test)]
    pub(crate) fn controller_time_phase(
        &self,
    ) -> crate::controller_time::ControllerTimeWorkerPhase {
        self.task
            .as_ref()
            .expect("pre-split task")
            .controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) fn controller_time_needs_recheck(&self) -> bool {
        self.task
            .as_ref()
            .expect("pre-split task")
            .controller_time_needs_recheck()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> InterruptBankOwner {
        self._interrupts
            .take()
            .expect("private Controller invariant retains the interrupt owner until activation")
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(&mut self) -> (&mut TaskResources, &mut P) {
        (
            self.task.as_mut().expect("pre-split task"),
            self._platform
                .as_mut()
                .expect("pre-split platform")
                .platform_mut(),
        )
    }

    /// Number of fixed modem timer slots retained by the initialized epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.runtime.modem_timer_capacity()
    }

    /// Return the scheduler scale retained by this exact hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Return the source-owned scheduler policy for this hardware epoch.
    pub const fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        self.config
    }

    /// Whether no software event has entered the initialized epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.runtime.is_pristine()
    }
}

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
impl ControllerPoweredTaskRuntime<'_> {
    #[cfg(target_arch = "riscv32")]
    pub fn request_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::ControllerTimeRequest,
        crate::controller_time::ControllerTimeRequestError,
    > {
        self._standalone_dtm_profile.gate_controller_time_request();
        self.task.request_controller_time()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller_time::ControllerTimeRequest,
    ) -> Result<(), crate::controller_time::ControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller_time::ControllerTimeRequest,
    ) -> Result<
        crate::controller_time::ControllerTimeEventStep,
        crate::controller_time::ControllerTimeEventError,
    > {
        self.task.recheck_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::ControllerTimeEventStep,
        crate::controller_time::ControllerTimeEventError,
    > {
        self.task.drain_orphan_controller_time()
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize> SchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
    /// Claim the task HAL slot once while borrowing stable software workers.
    ///
    /// The task endpoint carries an exclusive, non-cloneable lease. HCI
    /// retirement later extracts the actual register and controller-time owners.
    /// The fourth endpoint leases the platform separately; command states never
    /// contain its generic type. Interrupt publications and software queues stay borrowed.
    /// A later split returns `None`, including after a runtime was dropped;
    /// dropping it neither restores hardware nor authorizes a new epoch.
    pub fn split_runtime(
        &mut self,
    ) -> Option<(
        ControllerInterruptRuntime<'_>,
        ControllerPoweredTaskRuntime<'_>,
        ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY>,
        crate::resources::platform_retirement::ControllerPlatformLease<'_, P>,
    )> {
        let task = self.task.lease()?;
        let platform = crate::resources::platform_retirement::ControllerPlatformLease::claim(
            &mut self._platform,
        )
        .expect("task and platform are claimed in one split");
        let time_scale = self.time_scale;
        let standalone_dtm_profile = &self._standalone_dtm_profile;
        let config = self.config;
        let (interrupt, software, modem_timer) = self.runtime.split();
        Some((
            interrupt,
            ControllerPoweredTaskRuntime::new(
                software,
                task,
                time_scale,
                standalone_dtm_profile,
                config,
            ),
            modem_timer,
            platform,
        ))
    }
}

impl<P> ControllerHalInitialized<P> {
    /// Initialize scheduler hardware and bind one static no-RTOS runtime.
    ///
    /// This consumes the completed controller HAL state before the first
    /// scheduler-table write. The supplied runtime must be pristine and is
    /// consumed into the same powered ownership epoch; it replaces the vendor
    /// event, broker-node and task containers instead of emulating their ABI.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_scheduler<const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |task| task.clear_scheduler_hardware_list_heads())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn initialize_scheduler_for_validation<const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |_| {
            BluetoothSchedulerHardwareListsCleared::for_validation()
        })
    }

    fn initialize_scheduler_with<const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
        initialize_hardware: impl FnOnce(&mut TaskResources) -> BluetoothSchedulerHardwareListsCleared,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
        assert!(
            runtime.is_pristine(),
            "only a pristine Controller runtime can initialize a scheduler epoch"
        );
        let Self {
            mut task,
            interrupts,
            platform,
            time_scale,
            standalone_dtm_profile,
        } = self;
        let hardware_lists_cleared = initialize_hardware(&mut task);
        SchedulerInitialized {
            task: crate::resources::runtime_owner::RuntimeOwnerSlot::new(task),
            _interrupts: Some(interrupts),
            _platform: crate::resources::runtime_owner::RuntimeOwnerSlot::new(platform),
            time_scale,
            _standalone_dtm_profile: standalone_dtm_profile,
            config: SchedulerSoftwareConfig::reviewed_standalone(),
            _hardware_lists_cleared: hardware_lists_cleared,
            runtime,
        }
    }
}

#[cfg(test)]
mod tests;
