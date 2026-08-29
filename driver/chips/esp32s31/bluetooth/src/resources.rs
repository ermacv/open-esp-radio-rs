//! Lossless ownership transitions for standalone Bluetooth hardware.

#[cfg(any(target_arch = "riscv32", test))]
use core::mem::ManuallyDrop;

use open_esp_radio_esp32s31_hal::BluetoothColdOwner as HalBluetoothColdOwner;
#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use open_esp_radio_esp32s31_hal::BluetoothControllerHalBorrow;
#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareListsCleared;
#[cfg(test)]
use open_esp_radio_esp32s31_hal::BluetoothTaskOwnerReuniteFailure;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptOutputPreparedOwner, BluetoothModemLpTimerLowPowerHardwareInitializedOwner,
    BluetoothModemLpTimerOwnerError, BluetoothPhyRegisterInitInputs,
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerRunEventPublished, BluetoothSchedulerRunInterruptsPrepared,
};
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptSetupOwner as HalBluetoothInterruptSetupOwner,
    BluetoothTaskOwner as HalBluetoothTaskOwner,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::{BluetoothSharedPhyBorrow, SharedPhyHal};
#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;
use open_esp_radio_esp32s31_pac::RadioHardware;
use open_esp_radio_esp32s31_pac::RadioPhyReleaseError;

#[cfg(target_arch = "riscv32")]
use crate::controller_time::{
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError,
};
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
use crate::controller_time::{BluetoothControllerTimeWorker, BluetoothControllerTimeWorkerPhase};

/// Complete cold Bluetooth owner before any powered lifecycle transaction.
///
/// This is the only public entry to and exit from the standalone Bluetooth
/// lifecycle. Keeping the platform lease and protocol-neutral radio root in
/// one affine value prevents owners from unrelated epochs being paired during
/// clock enable, rollback, or a later Wi-Fi handoff.
#[must_use = "stopped Bluetooth retains the platform and radio owners"]
pub struct BluetoothStopped<P> {
    registers: HalBluetoothColdOwner,
    platform: P,
}

/// Failed stopped-route transition retaining the complete Bluetooth owner.
#[must_use = "failed Bluetooth route transition retains the platform and radio owners"]
pub struct BluetoothStoppedReleaseFailure<P> {
    stopped: BluetoothStopped<P>,
    error: RadioPhyReleaseError,
}

impl<P> BluetoothStoppedReleaseFailure<P> {
    pub const fn error(&self) -> RadioPhyReleaseError {
        self.error
    }

    pub fn into_parts(self) -> (BluetoothStopped<P>, RadioPhyReleaseError) {
        (self.stopped, self.error)
    }
}

impl<P> core::fmt::Debug for BluetoothStoppedReleaseFailure<P> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothStoppedReleaseFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<P> BluetoothStopped<P> {
    /// Bind one platform lease to the exact protocol-neutral radio root.
    ///
    /// This transition performs no MMIO. Once constructed, the pair can only
    /// move together through the typed Bluetooth lifecycle.
    pub fn from_hardware(platform: P, hardware: RadioHardware) -> Self {
        Self {
            registers: HalBluetoothColdOwner::from_radio_hardware(hardware),
            platform,
        }
    }

    /// Release an unpowered Bluetooth owner for another radio protocol.
    ///
    /// # Errors
    ///
    /// Returns [`BluetoothStoppedReleaseFailure`] retaining the complete
    /// stopped owner when TX-DC PWDET fields still await restoration.
    pub fn release(self) -> Result<(P, RadioHardware), BluetoothStoppedReleaseFailure<P>> {
        let Self {
            registers,
            platform,
        } = self;
        match registers.release() {
            Ok(hardware) => Ok((platform, hardware)),
            Err(failure) => {
                let (registers, error) = failure.into_parts();
                Err(BluetoothStoppedReleaseFailure {
                    stopped: Self {
                        registers,
                        platform,
                    },
                    error,
                })
            }
        }
    }

    pub(crate) fn into_parts(self) -> (HalBluetoothColdOwner, P) {
        (self.registers, self.platform)
    }

    pub(crate) const fn from_parts(registers: HalBluetoothColdOwner, platform: P) -> Self {
        Self {
            registers,
            platform,
        }
    }
}

/// Platform owner retained after the first non-reversible powered mutation.
///
/// Once PHY or controller initialization has started, releasing the ordinary
/// platform reservation would advertise a reusable Bluetooth lifecycle even
/// though no verified hardware teardown occurred. This wrapper deliberately
/// suppresses `P::drop`; a future verified teardown transaction must recover
/// the platform owner and release the reservation.
#[must_use = "the powered platform remains retained until verified PHY teardown"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothTeardownPendingPlatform<P> {
    _platform: ManuallyDrop<P>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<P> BluetoothTeardownPendingPlatform<P> {
    pub(crate) const fn new(platform: P) -> Self {
        Self {
            _platform: ManuallyDrop::new(platform),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn platform_mut(&mut self) -> &mut P {
        &mut self._platform
    }
}

/// Separate the cold HAL owner into the controller lifecycle's task and IRQ
/// owners without exposing either partition publicly.
///
/// This transition performs no MMIO. In particular it does not configure
/// controller masks or a CPU interrupt route.
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) fn separate_interrupt_owner(
    registers: HalBluetoothColdOwner,
) -> (BluetoothTaskResources, BluetoothInterruptBankOwner) {
    let (task, interrupts) = registers.separate_interrupt_owner();
    (
        BluetoothTaskResources {
            registers: task,
            controller_time: BluetoothControllerTimeWorker::new_idle(),
        },
        BluetoothInterruptBankOwner {
            _registers: interrupts,
        },
    )
}

/// Ordinary task-side owner of the standalone Bluetooth controller region.
///
/// No MMIO operation is exposed until its finite lifecycle transaction has
/// independent vendor evidence.
#[must_use = "the Bluetooth task owner must be reunited before release"]
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) struct BluetoothTaskResources {
    registers: HalBluetoothTaskOwner,
    controller_time: BluetoothControllerTimeWorker,
}

#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
impl BluetoothTaskResources {
    /// Execute the source-127 register prefix and following complete low-power
    /// hardware component while the upper lifecycle retains initialized
    /// Controller software and an inactive route.
    ///
    /// # Safety
    ///
    /// The caller must own the matching powered scheduler/HCI epoch, must not
    /// have installed source 127, and must retain the returned disjoint timer
    /// owner until verified route teardown.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the upper lifecycle proves the powered software and inactive-route prerequisites"
    )]
    pub(crate) unsafe fn initialize_modem_lp_timer_hardware(
        &mut self,
    ) -> Result<
        BluetoothModemLpTimerLowPowerHardwareInitializedOwner,
        BluetoothModemLpTimerOwnerError,
    > {
        let prepared = unsafe { self.registers.prepare_modem_lp_timer_registers()? };
        Ok(unsafe { prepared.initialize_low_power_hardware(&mut self.registers) })
    }

    /// Remove every scheduler hardware-list head through one finite HAL borrow.
    #[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
    pub(crate) fn clear_scheduler_hardware_list_heads(
        &mut self,
    ) -> BluetoothSchedulerHardwareListsCleared {
        self.registers
            .borrow_bluetooth_controller()
            .clear_scheduler_hardware_list_heads()
    }

    /// Publish one already initialized scheduler graph through the sole task
    /// register owner retained by this powered epoch.
    ///
    /// # Safety
    ///
    /// The caller must retain the matching exclusive scheduler-list epoch and
    /// the complete pinned graph, must have finished every descriptor write,
    /// and must prove that no interrupt-side scheduler access can race this
    /// operation. The lower PAC orders descriptor visibility before MMIO.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the scheduler lifecycle discharges graph lifetime and inactive-route prerequisites"
    )]
    pub(crate) unsafe fn publish_scheduler_hardware_list_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        let mut controller = self.registers.borrow_bluetooth_controller();
        unsafe { controller.publish_scheduler_hardware_list_head(index, head) }
    }

    /// Publish the synchronous BTMAC scheduler event after the exact head and
    /// interrupt preparation have completed.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        self.registers
            .borrow_bluetooth_controller()
            .publish_scheduler_run_event(head, interrupts)
    }

    /// Consume the complete run-event proof into the final hardware RUN
    /// publication.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn publish_scheduler_hardware_run_command(
        &mut self,
        event: BluetoothSchedulerRunEventPublished,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        self.registers
            .borrow_bluetooth_controller()
            .publish_scheduler_hardware_run_command(event)
    }

    /// Durable logical phase paired with this unique task owner.
    pub(crate) const fn controller_time_phase(&self) -> BluetoothControllerTimeWorkerPhase {
        self.controller_time.phase()
    }

    /// Whether the runner must retain a durable recheck event or deadline.
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.controller_time.needs_recheck()
    }

    /// Publish one request while retaining worker and PAC ownership together.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn request_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError> {
        let Self {
            registers,
            controller_time,
        } = self;
        let mut controller = registers.borrow_bluetooth_controller();
        controller_time.request(&mut controller)
    }

    /// Abandon only the matching logical request; late cancellation is inert.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn abandon_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> bool {
        self.controller_time.abandon(request)
    }

    /// Perform one bounded hardware recheck with a short HAL borrow.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimeEventStep, BluetoothControllerTimeEventError> {
        let Self {
            registers,
            controller_time,
        } = self;
        let mut controller = registers.borrow_bluetooth_controller();
        controller_time.on_recheck_event(&mut controller)
    }

    /// Advance one scheduler lock/modify worker with this exact task-side HAL
    /// owner and no exported register capability.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn step_scheduler_lock_modify(
        &mut self,
        worker: &mut crate::BluetoothSchedulerLockModifyWorker,
        event: crate::BluetoothSchedulerLockModifyEvent,
    ) -> crate::BluetoothSchedulerLockModifyWorkerStep {
        let mut controller = self.registers.borrow_bluetooth_controller();
        worker.step(event, &mut controller)
    }

    /// Capture one fenced finished-list transfer into the sole bounded worker.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn capture_scheduler_finished_lists(
        &mut self,
        worker: &mut crate::BluetoothSchedulerFinishedListWorker,
    ) -> Result<(), crate::BluetoothSchedulerFinishedListCaptureError> {
        let mut controller = self.registers.borrow_bluetooth_controller();
        worker.capture(&mut controller)
    }

    /// Perform one fresh fenced hardware-head retirement observation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_scheduler_hardware_list_head_retirement(
        &mut self,
        run: open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareRunCommandPublished,
    ) -> open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareListHeadRetirementObservation {
        self.registers
            .borrow_bluetooth_controller()
            .observe_scheduler_hardware_list_head_retirement(run)
    }

    /// Execute the complete reviewed controller HAL-init component.
    ///
    /// The owning lifecycle invokes this component after clocks and before
    /// scheduler initialization. Later event/list, interrupt, PHY, BTBB and
    /// BLE stages remain separate prerequisites for a running controller.
    ///
    /// # Safety
    ///
    /// The caller must retain every prerequisite documented by the PAC
    /// transaction and must not infer controller or HCI readiness from return.
    #[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
    #[allow(
        unsafe_code,
        reason = "the unsafe bridge retains the controller HAL-init clock and IRQ prerequisites"
    )]
    #[allow(
        dead_code,
        reason = "only the target lifecycle and isolated validation probes invoke this component"
    )]
    pub(crate) unsafe fn initialize_controller_hal(
        &mut self,
        config: BluetoothControllerHalInitConfig,
    ) {
        unsafe {
            self.registers.initialize_controller_hal_transaction(config);
        }
    }

    /// Borrow the protocol-neutral radio PHY for one finite lower-layer scope.
    ///
    /// The returned HAL derives shared baseband state from the route PAC;
    /// selecting the Bluetooth route alone is not treated as proof that the
    /// shared settle condition is false.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn shared_phy_hal(&mut self) -> SharedPhyHal<'_> {
        self.registers.borrow_shared_phy()
    }

    /// Execute the reviewed finite BT baseband-v2 initialization transaction.
    ///
    /// This crate-private bridge exists so only the lifecycle typestate can
    /// reach the PAC transaction in an ordinary production build.
    ///
    /// # Safety
    ///
    /// The caller must retain the matching completed common-PHY owner, derive
    /// `gain_parameter` from that terminal state, and preserve every hardware
    /// owner until verified last-owner teardown.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the unsafe signature retains the PAC common-PHY prerequisite across the crate boundary"
    )]
    pub(crate) unsafe fn initialize_baseband_v2(&mut self, gain_parameter: u8) {
        unsafe {
            self.registers
                .initialize_baseband_v2_arg_one(gain_parameter);
        }
    }

    /// Publish the complete BLE PHY register-init transaction for a lifecycle
    /// that retains both address-bound storage objects.
    ///
    /// # Safety
    ///
    /// The caller must retain the exact completed common-PHY and BTBB owner,
    /// the inactive interrupt bank, and the storage represented by `inputs`
    /// until all controller consumers are stopped by a verified transition.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the upper typestate retains the complete PAC lifecycle and storage prerequisites"
    )]
    pub(crate) unsafe fn initialize_ble_phy_registers(
        &mut self,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        unsafe {
            self.registers.initialize_ble_phy_registers(inputs);
        }
    }

    /// Reunite a quiescent task owner with its inactive interrupt owner.
    #[cfg(test)]
    pub(crate) fn reunite(
        self,
        interrupts: BluetoothInterruptBankOwner,
    ) -> Result<HalBluetoothColdOwner, BluetoothTaskOwnerReuniteFailure> {
        assert!(
            self.controller_time.is_reunitable(),
            "controller-time fault or transaction prevents cold reunion"
        );
        self.registers.into_cold(interrupts._registers)
    }
}

/// Inactive owner of the Bluetooth controller interrupt bank.
#[must_use = "the interrupt owner must be installed or reunited"]
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
pub(crate) struct BluetoothInterruptBankOwner {
    _registers: HalBluetoothInterruptSetupOwner,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothInterruptBankOwner {
    /// Prepare the controller output while retaining the affine IRQ partition.
    ///
    /// # Safety
    ///
    /// The caller must own the matching completed Controller initialization
    /// and prove that all three CPU routes remain inactive.
    #[allow(
        unsafe_code,
        reason = "the upper Controller typestate discharges the HAL interrupt prerequisites"
    )]
    pub(crate) unsafe fn prepare_controller_output(self) -> BluetoothInterruptOutputPreparedOwner {
        // SAFETY: the caller retains the complete matching Controller epoch
        // and the only route installers are still inaccessible.
        unsafe { self._registers.prepare_controller_output() }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use open_esp_radio_esp32s31_hal::SharedPhyAccess;
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use crate::controller_time::BluetoothControllerTimeWorkerPhase;

    use super::{BluetoothStopped, BluetoothTeardownPendingPlatform, separate_interrupt_owner};

    static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

    struct PlatformDropCounter;

    impl Drop for PlatformDropCounter {
        fn drop(&mut self) {
            PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn pending_phy_teardown_suppresses_implicit_platform_release() {
        PLATFORM_DROPS.store(0, Ordering::Relaxed);
        drop(BluetoothTeardownPendingPlatform::new(PlatformDropCounter));
        assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn task_and_interrupt_owners_reunite_into_the_same_radio_root() {
        let stopped = BluetoothStopped::from_hardware((), RadioHardware::for_validation());
        let (registers, ()) = stopped.into_parts();
        let (task, setup) = separate_interrupt_owner(registers);
        assert_eq!(
            task.controller_time_phase(),
            BluetoothControllerTimeWorkerPhase::Idle
        );
        let hardware = task
            .reunite(setup)
            .expect("untouched owners remain cold-reunitable")
            .release()
            .expect("an untouched Bluetooth route can be released");

        // Re-entering Wi-Fi proves that every inactive protocol and shared
        // owner survived the complete Bluetooth ownership roundtrip.
        let _wifi = hardware.into_wifi();
    }

    #[test]
    fn mutable_shared_phy_borrow_arms_fail_stop_reunion() {
        fn accepts_shared_phy(_: &mut impl SharedPhyAccess) {}

        let stopped = BluetoothStopped::from_hardware((), RadioHardware::for_validation());
        let (registers, ()) = stopped.into_parts();
        let (mut task, setup) = separate_interrupt_owner(registers);
        {
            let mut phy = task.shared_phy_hal();
            accepts_shared_phy(&mut phy);
        }

        let failure = match task.reunite(setup) {
            Ok(_) => panic!("a mutable shared-PHY borrow requires verified rollback"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            open_esp_radio_esp32s31_hal::BluetoothTaskOwnerReuniteError::HardwareLifecycleNotRestored
        );
    }
}
