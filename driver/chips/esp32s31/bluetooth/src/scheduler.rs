//! Fact-bounded scheduler initialization after the controller HAL component.

use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerRuntimeResources,
    BluetoothControllerTaskRuntime,
    controller_hal::BluetoothControllerHalInitialized,
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
    },
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

/// Source-owned scheduler policy copied by the reviewed controller init path.
///
/// The current archive proves two complete integer values but does not expose
/// their physical units or independent field meanings. Keeping them in a
/// software type prevents the vendor's private eight-byte structure layout
/// from becoming part of the open Controller ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerSoftwareConfig {
    value_0: u32,
    value_1: u32,
}

impl BluetoothSchedulerSoftwareConfig {
    /// Configuration constructed by the complete ESP32-S31 standalone task.
    pub const fn reviewed_standalone() -> Self {
        Self {
            value_0: 40,
            value_1: 46,
        }
    }

    /// First positional scheduler policy value.
    pub const fn value_0(self) -> u32 {
        self.value_0
    }

    /// Second positional scheduler policy value.
    pub const fn value_1(self) -> u32 {
        self.value_1
    }
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
/// The open scheduler item queue, remaining hardware initialization and stable
/// ISR publication are still missing, so this state exposes no PHY, BTBB, IRQ,
/// HCI, Controller or Link-Layer readiness. Dropping it is fail-stop because
/// no verified rollback exists after scheduler MMIO mutation.
#[must_use = "the initialized scheduler retains every powered Bluetooth owner"]
pub struct BluetoothSchedulerInitialized<P, const MODEM_TIMER_CAPACITY: usize> {
    _task: BluetoothTaskResources,
    _interrupts: BluetoothInterruptBankOwner,
    _platform: BluetoothTeardownPendingPlatform<P>,
    time_scale: BluetoothControllerTimeScale,
    config: BluetoothSchedulerSoftwareConfig,
    runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
}

impl<P, const MODEM_TIMER_CAPACITY: usize> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
    /// Number of fixed modem timer slots retained by the initialized epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.runtime.modem_timer_capacity()
    }

    /// Return the scheduler scale retained by this exact hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Return the source-owned scheduler policy for this hardware epoch.
    pub const fn scheduler_config(&self) -> BluetoothSchedulerSoftwareConfig {
        self.config
    }

    /// Whether no software event has entered the initialized epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.runtime.is_pristine()
    }

    /// Borrow the matching interrupt and task runtime endpoints from this
    /// initialized hardware epoch.
    ///
    /// This is the production entry into an executor adapter. The retained
    /// task, interrupt and platform owners cannot move or be rebound while
    /// either endpoint is alive.
    pub fn split_runtime(
        &mut self,
    ) -> (
        BluetoothControllerInterruptRuntime<'_>,
        BluetoothControllerTaskRuntime<'_>,
    ) {
        self.runtime.split()
    }
}

impl<P> BluetoothControllerHalInitialized<P> {
    /// Initialize scheduler hardware and bind one static no-RTOS runtime.
    ///
    /// This consumes the completed controller HAL state before the first
    /// scheduler-table write. The supplied runtime must be pristine and is
    /// consumed into the same powered ownership epoch; it replaces the vendor
    /// event, broker-node and task containers instead of emulating their ABI.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_scheduler<const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
    ) -> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |task| {
            task.clear_scheduler_hardware_list_heads();
        })
    }

    fn initialize_scheduler_with<const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
        initialize_hardware: impl FnOnce(&mut BluetoothTaskResources),
    ) -> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY> {
        assert!(
            runtime.is_pristine(),
            "only a pristine Controller runtime can initialize a scheduler epoch"
        );
        let Self {
            mut task,
            interrupts,
            platform,
            time_scale,
        } = self;
        initialize_hardware(&mut task);
        BluetoothSchedulerInitialized {
            _task: task,
            _interrupts: interrupts,
            _platform: platform,
            time_scale,
            config: BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            runtime,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use open_esp_radio_esp32s31_pac::RadioHardware;

    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use crate::{BluetoothClockedResources, BluetoothControllerRuntimeResources, BluetoothStopped};

    static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

    struct FakePlatform;

    impl Drop for FakePlatform {
        fn drop(&mut self) {
            PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn controller_hal_precedes_complete_scheduler_init_and_arms_fail_stop() {
        PLATFORM_DROPS.store(0, Ordering::Relaxed);
        let stopped =
            BluetoothStopped::from_hardware(FakePlatform, RadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let operations = Rc::new(RefCell::new(Vec::new()));
        let hal_operations = Rc::clone(&operations);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {
            hal_operations.borrow_mut().push("controller-hal");
        });
        let time_scale = initialized.controller_time_scale();
        let scheduler_operations = Rc::clone(&operations);
        let mut scheduler = initialized.initialize_scheduler_with(
            BluetoothControllerRuntimeResources::<4>::new(),
            |_| {
                scheduler_operations.borrow_mut().push("scheduler-hardware");
            },
        );
        assert_eq!(
            operations.borrow().as_slice(),
            ["controller-hal", "scheduler-hardware"]
        );
        assert_eq!(scheduler.controller_time_scale(), time_scale);
        assert_eq!(scheduler.modem_timer_capacity(), 4);
        assert!(scheduler.runtime_is_pristine());
        let (interrupt, task) = scheduler.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        drop((interrupt, task));
        drop(scheduler);
        assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
    }
}
