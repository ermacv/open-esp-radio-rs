//! Hardware low-power boot and post-publication HCI binding.

#[cfg(any(target_arch = "riscv32", test))]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_bluetooth_hci::LeControllerHciResources;

use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerModemTimerRuntime,
    BluetoothControllerPoweredTaskRuntime, BluetoothSchedulerInitialized,
    BluetoothSchedulerSoftwareConfig,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothLowPowerRuntimeControlObservation,
    BluetoothModemLpTimerLowPowerHardwareInitializedOwner, BluetoothModemLpTimerOwnerError,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

/// Rejection reason for post-publication HCI binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum BluetoothControllerHciBindError {
    /// The supplied HCI queues already contain state from another runtime epoch.
    ResourcesNotPristine,
}

#[cfg(any(target_arch = "riscv32", test))]
fn validate_hci_bind<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>(
    hci: &LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
) -> Result<(), BluetoothControllerHciBindError>
where
    M: RawMutex,
{
    if hci.is_pristine() {
        Ok(())
    } else {
        Err(BluetoothControllerHciBindError::ResourcesNotPristine)
    }
}

/// Lossless post-publication HCI binding failure.
#[must_use = "failed HCI binding returns both the hardware and protocol owners"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerHciBindFailure<
    P,
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
    hardware: crate::BluetoothControllerInterruptOwnersPublished<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    error: BluetoothControllerHciBindError,
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciBindFailure<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Exact reason why the protocol resources could not join this hardware epoch.
    pub const fn error(&self) -> BluetoothControllerHciBindError {
        self.error
    }

    /// Recover both unchanged affine owners.
    pub fn into_parts(
        self,
    ) -> (
        crate::BluetoothControllerInterruptOwnersPublished<
            P,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
        >,
        LeControllerHciResources<
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        (self.hardware, self.hci)
    }
}

/// Final published hardware owner joined to one pristine HCI protocol epoch.
#[must_use = "the final Bluetooth Controller owner must remain in stable storage"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothControllerHciBound<
    P,
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
    hardware: crate::BluetoothControllerInterruptOwnersPublished<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

#[cfg(target_arch = "riscv32")]
impl<P, S, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    crate::BluetoothControllerInterruptOwnersPublished<
        P,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
{
    /// Bind pristine HCI protocol resources after all interrupt owners reached stable storage.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure returns both complete affine owners"
    )]
    pub fn bind_hci<
        M,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        hci: LeControllerHciResources<
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<
        BluetoothControllerHciBound<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerHciBindFailure<
            P,
            M,
            S,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    >
    where
        M: RawMutex,
    {
        match validate_hci_bind(&hci) {
            Ok(()) => Ok(BluetoothControllerHciBound {
                hardware: self,
                hci,
            }),
            Err(error) => Err(BluetoothControllerHciBindFailure {
                hardware: self,
                hci,
                error,
            }),
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<
    P,
    M,
    S,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciBound<
        P,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
    S: crate::BluetoothModemLpTimerSoftwareOwnerStorage,
{
    /// Split one stable final owner into routed runtime endpoints.
    pub fn split_runtime<'runtime>(
        &'runtime mut self,
    ) -> crate::BluetoothControllerPublishedRuntimeSplit<
        'runtime,
        M,
        S,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let Self { hardware, hci } = self;
        hardware.split_hardware_runtime().bind_hci(hci.split())
    }
}

/// Powered Controller ownership after the complete modem low-power hardware
/// component and before source 127 is installed.
///
/// The disjoint timer owner stays inside this state. It is not yet in stable
/// ISR storage, no CPU route is enabled, and this state therefore exposes no
/// live timer interrupt or operational Link-Layer capability.
#[must_use = "the initialized low-power hardware retains every powered Bluetooth owner"]
pub struct BluetoothControllerLowPowerHardwareInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    scheduler: BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    timer_hardware: Option<BluetoothModemLpTimerLowPowerHardwareInitializedOwner>,
}

/// Lossless failure to initialize the disjoint modem low-power timer owner.
#[must_use = "the powered scheduler remains owned after a rejected transition"]
pub struct BluetoothControllerLowPowerHardwareInitializationFailure<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    scheduler: BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    error: BluetoothModemLpTimerOwnerError,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerLowPowerHardwareInitializationFailure<
        P,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
    >
{
    /// Exact lower ownership error.
    pub const fn error(&self) -> BluetoothModemLpTimerOwnerError {
        self.error
    }

    /// Recover the complete powered Controller owner.
    pub fn into_scheduler(
        self,
    ) -> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        self.scheduler
    }
}

/// All borrowed runtime endpoints from one powered Controller epoch.
///
/// Named fields keep interrupt, task and modem-timer roles explicit. HCI is not
/// part of this hardware-only split and is joined only after stable interrupt
/// publication.
#[must_use = "all runtime endpoints belong to one powered Controller epoch"]
pub struct BluetoothControllerRuntimeEndpoints<
    'runtime,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    /// Interrupt-side scheduler publications.
    pub interrupt: BluetoothControllerInterruptRuntime<'runtime>,
    /// Task-side scheduler workers.
    pub task: BluetoothControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    /// Unique mutable source-127 queue and epoch runtime.
    pub modem_timer: BluetoothControllerModemTimerRuntime<'runtime, MODEM_TIMER_CAPACITY>,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc test seam returns the complete affine powered owner"
    )]
    fn try_initialize_low_power_hardware_with<TimerHardware, Error>(
        mut self,
        initialize: impl FnOnce(
            &mut crate::resources::BluetoothTaskResources,
        ) -> Result<TimerHardware, Error>,
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
        BluetoothControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        BluetoothControllerLowPowerHardwareInitializationFailure<
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
            Ok((scheduler, timer_hardware)) => Ok(BluetoothControllerLowPowerHardwareInitialized {
                scheduler,
                timer_hardware: Some(timer_hardware),
            }),
            Err((scheduler, error)) => {
                Err(BluetoothControllerLowPowerHardwareInitializationFailure { scheduler, error })
            }
        }
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerLowPowerHardwareInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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
    pub const fn scheduler_config(&self) -> BluetoothSchedulerSoftwareConfig {
        self.scheduler.scheduler_config()
    }

    /// Whether no scheduler software work entered this epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.scheduler.runtime_is_pristine()
    }

    /// Borrow all matching hardware endpoints without releasing the
    /// retained timer-hardware owner.
    pub fn split_runtime(
        &mut self,
    ) -> BluetoothControllerRuntimeEndpoints<'_, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        let (interrupt, task, modem_timer) = self.scheduler.split_runtime();
        BluetoothControllerRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(
        &mut self,
    ) -> (&mut crate::resources::BluetoothTaskResources, &mut P) {
        self.scheduler.common_phy_parts_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn task_mut(&mut self) -> &mut crate::resources::BluetoothTaskResources {
        self.scheduler.task_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> crate::resources::BluetoothInterruptBankOwner {
        self.scheduler.take_interrupt_owner()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_timer_hardware(
        &mut self,
    ) -> BluetoothModemLpTimerLowPowerHardwareInitializedOwner {
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
mod tests {
    use crate::{
        BluetoothClockedResources, BluetoothControllerRuntimeResources, BluetoothRadioHardware,
        BluetoothSchedulerInitialized, BluetoothStopped,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciEndpoints,
        LeControllerHciResources,
        bt_hci::{cmd::controller_baseband::Reset, transport::Transport},
    };

    use super::{BluetoothControllerHciBindError, validate_hci_bind};

    fn scheduler() -> BluetoothSchedulerInitialized<(), 4, 3> {
        let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        BluetoothClockedResources::for_validation(registers, platform)
            .initialize_controller_hal_with(|_, _| {})
            .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::new())
    }

    fn hci() -> LeControllerHciResources<NoopRawMutex, 1, 1, 45> {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .expect("nonzero test profile");
        LeControllerHciResources::new(config).expect("profile fits its bounded queues")
    }

    #[test]
    fn post_publication_hci_binding_requires_a_pristine_epoch() {
        let pristine = hci();
        assert_eq!(validate_hci_bind(&pristine), Ok(()));

        let mut used = hci();
        {
            let LeControllerHciEndpoints {
                host,
                controller: _,
            } = used.split();
            block_on(async {
                host.write(&Reset::new())
                    .await
                    .expect("Reset enters the test queue");
            });
        }
        assert_eq!(
            validate_hci_bind(&used),
            Err(BluetoothControllerHciBindError::ResourcesNotPristine)
        );
    }

    #[test]
    fn scheduler_runtime_split_contains_only_hardware_services() {
        let mut scheduler = scheduler();
        assert!(scheduler.runtime_is_pristine());
        let (interrupt, task, modem_timer) = scheduler.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        assert!(core::ptr::eq(
            interrupt.modem_lp_timer_worker_wake(),
            modem_timer.worker_wake()
        ));
        assert!(modem_timer.queue_is_empty());
    }

    #[test]
    fn low_power_hardware_stays_in_the_same_pristine_controller_epoch() {
        let (mut controller, timer_hardware) = match scheduler()
            .try_initialize_low_power_hardware_with(|_| Ok::<_, ()>("timer-hardware"))
        {
            Ok(initialized) => initialized,
            Err(_) => panic!("the injected low-power component must complete"),
        };

        assert_eq!(timer_hardware, "timer-hardware");
        assert!(controller.runtime_is_pristine());
        assert_eq!(controller.modem_timer_capacity(), 4);
        let (interrupt, task, _) = controller.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
    }

    #[test]
    fn low_power_hardware_failure_returns_the_complete_scheduler_epoch() {
        let (controller, error) = match scheduler()
            .try_initialize_low_power_hardware_with(|_| Err::<(), _>("timer-owner-separated"))
        {
            Ok(_) => panic!("the injected lower failure must remain visible"),
            Err(failure) => failure,
        };

        assert_eq!(error, "timer-owner-separated");
        assert!(controller.runtime_is_pristine());
        assert_eq!(controller.modem_timer_capacity(), 4);
        assert_eq!(controller.scheduler_capacity(), 3);
    }
}
