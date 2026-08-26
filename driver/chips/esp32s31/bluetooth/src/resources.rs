//! Lossless ownership transitions for standalone Bluetooth hardware.

#[cfg(any(target_arch = "riscv32", test))]
use core::mem::ManuallyDrop;

use open_esp_radio_esp32s31_hal::BluetoothColdOwner as HalBluetoothColdOwner;
#[cfg(test)]
use open_esp_radio_esp32s31_hal::BluetoothTaskOwnerReuniteFailure;
#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerHalBorrow, BluetoothInterruptSetupOwner as HalBluetoothInterruptSetupOwner,
    BluetoothTaskOwner as HalBluetoothTaskOwner,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::{BluetoothSharedPhyBorrow, SharedPhyHal};
#[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;
use open_esp_radio_esp32s31_pac::RadioHardware;

#[cfg(any(target_arch = "riscv32", test, feature = "validation-probes"))]
use crate::controller_time::{
    BluetoothControllerTimeEventError, BluetoothControllerTimeEventStep,
    BluetoothControllerTimeRequest, BluetoothControllerTimeRequestError,
    BluetoothControllerTimeWorker, BluetoothControllerTimeWorkerPhase,
};

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
    pub fn release(self) -> (P, RadioHardware) {
        (self.platform, self.registers.release())
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
    /// Clear the reviewed scheduler-table prefix through one finite HAL borrow.
    #[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
    pub(crate) fn clear_scheduler_table_low_bits(&mut self) {
        self.registers
            .borrow_bluetooth_controller()
            .clear_scheduler_table_low_bits();
    }

    /// Durable logical phase paired with this unique task owner.
    #[allow(
        dead_code,
        reason = "the time worker awaits the powered controller runner composition"
    )]
    pub(crate) const fn controller_time_phase(&self) -> BluetoothControllerTimeWorkerPhase {
        self.controller_time.phase()
    }

    /// Whether the runner must retain a durable recheck event or deadline.
    #[allow(
        dead_code,
        reason = "the time worker awaits the powered controller runner composition"
    )]
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.controller_time.needs_recheck()
    }

    /// Publish one request while retaining worker and PAC ownership together.
    #[allow(
        dead_code,
        reason = "the time worker awaits the powered controller runner composition"
    )]
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
    #[allow(
        dead_code,
        reason = "the time worker awaits the powered controller runner composition"
    )]
    pub(crate) fn abandon_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> bool {
        self.controller_time.abandon(request)
    }

    /// Perform one bounded hardware recheck with a short HAL borrow.
    #[allow(
        dead_code,
        reason = "the time worker awaits the powered controller runner composition"
    )]
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

    /// Execute the complete reviewed controller HAL-init component.
    ///
    /// This bridge remains disconnected from ordinary production lifecycle
    /// until event/list initialization, the BLE enable stages and IRQ routing
    /// own the prerequisites between BTBB initialization and this transaction.
    ///
    /// # Safety
    ///
    /// The caller must retain every prerequisite documented by the PAC
    /// transaction and must not infer controller or HCI readiness from return.
    #[cfg(any(target_arch = "riscv32", feature = "validation-probes"))]
    #[allow(
        unsafe_code,
        reason = "the unsafe bridge retains the PAC controller HAL-init prerequisites"
    )]
    #[allow(
        dead_code,
        reason = "the verified component awaits composition across the missing BLE lifecycle stages"
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
            .release();

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
