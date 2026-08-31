//! Source-owned HCI initialization after the scheduler component.

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    LeControllerBootstrapConfig, LeControllerHciEndpoints, LeControllerHciResources,
};

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

/// Why a scheduler epoch could not acquire an HCI resource epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerHciInitializationError {
    /// A packet or successful bootstrap command already entered the supplied
    /// HCI resources.
    ResourcesNotPristine,
}

/// Lossless failure to bind scheduler and HCI resource ownership.
#[must_use = "the scheduler and HCI resources remain owned after a rejected bind"]
pub struct BluetoothControllerHciInitializationFailure<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    error: BluetoothControllerHciInitializationError,
    scheduler: BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciInitializationFailure<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Exact reason the affine bind was refused.
    pub const fn error(&self) -> BluetoothControllerHciInitializationError {
        self.error
    }

    /// Recover both unchanged owners after a rejected bind.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        LeControllerHciResources<
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        (self.scheduler, self.hci)
    }
}

/// Powered scheduler ownership with one source-owned HCI bootstrap epoch.
///
/// This state replaces the reviewed vendor HCI environment, packet mempool and
/// generic broker node with one bounded Rust aggregate. It proves only that
/// the Host transport and conservative bootstrap dispatcher belong to the
/// same powered Controller epoch. It exposes no advertising, scanning,
/// connection, ACL dataplane, Link Layer, PHY or on-air readiness.
#[must_use = "the initialized HCI state retains every powered Bluetooth owner"]
pub struct BluetoothControllerHciInitialized<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    scheduler: BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    hci: LeControllerHciResources<
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
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
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    controller: BluetoothControllerHciInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    timer_hardware: Option<BluetoothModemLpTimerLowPowerHardwareInitializedOwner>,
}

/// Lossless failure to initialize the disjoint modem low-power timer owner.
#[must_use = "the powered HCI Controller remains owned after a rejected transition"]
pub struct BluetoothControllerLowPowerHardwareInitializationFailure<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    controller: BluetoothControllerHciInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    error: BluetoothModemLpTimerOwnerError,
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerLowPowerHardwareInitializationFailure<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Exact lower ownership error.
    pub const fn error(&self) -> BluetoothModemLpTimerOwnerError {
        self.error
    }

    /// Recover the complete powered Controller owner.
    pub fn into_controller(
        self,
    ) -> BluetoothControllerHciInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        self.controller
    }
}

/// All borrowed runtime endpoints from one powered Controller epoch.
///
/// Named fields keep interrupt, task and HCI roles explicit. The mutable task
/// and combined Controller command endpoint prevent a second split while this
/// value is alive.
#[must_use = "all runtime endpoints belong to one powered Controller epoch"]
pub struct BluetoothControllerRuntimeEndpoints<
    'runtime,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Interrupt-side scheduler publications.
    pub interrupt: BluetoothControllerInterruptRuntime<'runtime>,
    /// Task-side scheduler workers.
    pub task: BluetoothControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    /// Unique mutable source-127 queue and epoch runtime.
    pub modem_timer: BluetoothControllerModemTimerRuntime<'runtime, MODEM_TIMER_CAPACITY>,
    /// Disjoint Host transport and combined Controller command endpoint.
    pub hci: LeControllerHciEndpoints<
        'runtime,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerHciInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
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

    /// Immutable HCI values reported to the Host during bootstrap.
    pub const fn hci_config(&self) -> LeControllerBootstrapConfig {
        self.hci.config()
    }

    /// Whether neither scheduler nor HCI software work entered this epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.scheduler.runtime_is_pristine() && self.hci.is_pristine()
    }

    /// Borrow all matching interrupt, task and HCI endpoints at once.
    pub fn split_runtime(
        &mut self,
    ) -> BluetoothControllerRuntimeEndpoints<
        '_,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let (interrupt, task, modem_timer) = self.scheduler.split_runtime();
        let hci = self.hci.split();
        BluetoothControllerRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
            hci,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(
        &mut self,
    ) -> (&mut crate::resources::BluetoothTaskResources, &mut P) {
        self.scheduler.common_phy_parts_mut()
    }

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
        match initialize(self.scheduler.task_mut()) {
            Ok(timer_hardware) => Ok((self, timer_hardware)),
            Err(error) => Err((self, error)),
        }
    }

    /// Execute the source-127 register prefix and complete modem low-power
    /// hardware initialization while the CPU route remains inactive.
    ///
    /// The safe Controller typestate discharges the lower HAL prerequisites:
    /// clocks, scheduler/HCI software and both disjoint register owners belong
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
        BluetoothControllerLowPowerHardwareInitialized<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerLowPowerHardwareInitializationFailure<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    > {
        match self.try_initialize_low_power_hardware_with(|task| {
            // SAFETY: this consuming state owns the powered scheduler/HCI
            // epoch and no public transition can have installed source 127.
            unsafe { task.initialize_modem_lp_timer_hardware() }
        }) {
            Ok((controller, timer_hardware)) => {
                Ok(BluetoothControllerLowPowerHardwareInitialized {
                    controller,
                    timer_hardware: Some(timer_hardware),
                })
            }
            Err((controller, error)) => {
                Err(BluetoothControllerLowPowerHardwareInitializationFailure { controller, error })
            }
        }
    }
}

impl<
    P,
    M,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    BluetoothControllerLowPowerHardwareInitialized<
        P,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Number of scheduler modem-timer slots retained by this exact epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.controller.modem_timer_capacity()
    }

    /// Number of source-owned scheduler reservations retained by this epoch.
    pub const fn scheduler_capacity(&self) -> usize {
        self.controller.scheduler_capacity()
    }

    /// Scheduler time scale retained by this exact powered epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.controller.controller_time_scale()
    }

    /// Source-owned scheduler policy retained by this exact powered epoch.
    pub const fn scheduler_config(&self) -> BluetoothSchedulerSoftwareConfig {
        self.controller.scheduler_config()
    }

    /// Immutable HCI values reported to the Host during bootstrap.
    pub const fn hci_config(&self) -> LeControllerBootstrapConfig {
        self.controller.hci_config()
    }

    /// Whether no scheduler or HCI software work entered this epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.controller.runtime_is_pristine()
    }

    /// Borrow all matching scheduler and HCI endpoints without releasing the
    /// retained timer-hardware owner.
    pub fn split_runtime(
        &mut self,
    ) -> BluetoothControllerRuntimeEndpoints<
        '_,
        M,
        MODEM_TIMER_CAPACITY,
        SCHEDULER_CAPACITY,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        self.controller.split_runtime()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(
        &mut self,
    ) -> (&mut crate::resources::BluetoothTaskResources, &mut P) {
        self.controller.common_phy_parts_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn task_mut(&mut self) -> &mut crate::resources::BluetoothTaskResources {
        self.controller.scheduler.task_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> crate::resources::BluetoothInterruptBankOwner {
        self.controller.scheduler.take_interrupt_owner()
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

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Bind a pristine bounded HCI bootstrap epoch after scheduler init.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return both affine powered owners"
    )]
    pub fn initialize_hci<
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
        BluetoothControllerHciInitialized<
            P,
            M,
            MODEM_TIMER_CAPACITY,
            SCHEDULER_CAPACITY,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        BluetoothControllerHciInitializationFailure<
            P,
            M,
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
        if !hci.is_pristine() {
            return Err(BluetoothControllerHciInitializationFailure {
                error: BluetoothControllerHciInitializationError::ResourcesNotPristine,
                scheduler: self,
                hci,
            });
        }
        Ok(BluetoothControllerHciInitialized {
            scheduler: self,
            hci,
        })
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciEndpoints,
        LeControllerHciResources,
        bt_hci::{cmd::controller_baseband::Reset, transport::Transport},
    };
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use crate::{
        BluetoothClockedResources, BluetoothControllerHciInitializationError,
        BluetoothControllerRuntimeEndpoints, BluetoothControllerRuntimeResources,
        BluetoothSchedulerInitialized, BluetoothStopped,
    };

    type TestHciResources = LeControllerHciResources<NoopRawMutex, 1, 1, 31>;

    fn scheduler() -> BluetoothSchedulerInitialized<(), 4, 3> {
        let stopped = BluetoothStopped::from_hardware((), RadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        BluetoothClockedResources::for_validation(registers, platform)
            .initialize_controller_hal_with(|_, _| {})
            .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::new())
    }

    fn hci() -> TestHciResources {
        let config = LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .expect("nonzero test profile");
        TestHciResources::new(config).expect("profile fits its bounded queues")
    }

    #[test]
    fn scheduler_and_hci_split_as_one_pristine_powered_epoch() {
        let mut controller = scheduler()
            .initialize_hci(hci())
            .unwrap_or_else(|_| panic!("pristine HCI resources must bind"));
        assert!(controller.runtime_is_pristine());
        assert_eq!(controller.modem_timer_capacity(), 4);
        assert_eq!(controller.scheduler_capacity(), 3);

        let BluetoothControllerRuntimeEndpoints {
            interrupt,
            task,
            modem_timer,
            hci,
        } = controller.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        assert!(core::ptr::eq(
            interrupt.modem_lp_timer_worker_wake(),
            modem_timer.worker_wake()
        ));
        assert!(modem_timer.queue_is_empty());
        let LeControllerHciEndpoints { host, controller } = hci;
        block_on(async {
            host.write(&Reset::new())
                .await
                .expect("Reset enters the matching combined HCI epoch");
        });
        assert_eq!(
            controller.bootstrap_phase(),
            open_esp_radio_bluetooth_hci::BootstrapPhase::AwaitingReset
        );
    }

    #[test]
    fn used_hci_epoch_is_rejected_without_losing_either_owner() {
        let mut used_hci = hci();
        {
            let LeControllerHciEndpoints {
                host,
                controller: _,
            } = used_hci.split();
            block_on(async {
                host.write(&Reset::new())
                    .await
                    .expect("Reset enters the test queue");
            });
        }

        let failure = match scheduler().initialize_hci(used_hci) {
            Ok(_) => panic!("a used HCI epoch must not bind"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothControllerHciInitializationError::ResourcesNotPristine
        );
        let (scheduler, hci) = failure.into_parts();
        assert!(scheduler.runtime_is_pristine());
        assert!(!hci.is_pristine());
    }

    #[test]
    fn low_power_hardware_stays_in_the_same_pristine_controller_epoch() {
        let controller = scheduler()
            .initialize_hci(hci())
            .unwrap_or_else(|_| panic!("pristine HCI resources must bind"));
        let (mut controller, timer_hardware) = match controller
            .try_initialize_low_power_hardware_with(|_| Ok::<_, ()>("timer-hardware"))
        {
            Ok(initialized) => initialized,
            Err(_) => panic!("the injected low-power component must complete"),
        };

        assert_eq!(timer_hardware, "timer-hardware");
        assert!(controller.runtime_is_pristine());
        assert_eq!(controller.modem_timer_capacity(), 4);
        let endpoints = controller.split_runtime();
        assert!(core::ptr::eq(
            endpoints.interrupt.scheduler_wake(),
            endpoints.task.scheduler_wake()
        ));
    }

    #[test]
    fn low_power_hardware_failure_returns_the_complete_hci_epoch() {
        let controller = scheduler()
            .initialize_hci(hci())
            .unwrap_or_else(|_| panic!("pristine HCI resources must bind"));
        let (controller, error) = match controller
            .try_initialize_low_power_hardware_with(|_| Err::<(), _>("timer-owner-separated"))
        {
            Ok(_) => panic!("the injected lower failure must remain visible"),
            Err(failure) => failure,
        };

        assert_eq!(error, "timer-owner-separated");
        assert!(controller.runtime_is_pristine());
        assert_eq!(controller.hci_config(), hci().config());
    }
}
