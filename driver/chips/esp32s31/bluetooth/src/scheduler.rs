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

/// Bluetooth hardware after all sixteen scheduler hardware-list heads were
/// removed.
///
/// This is a deliberately terminal research frontier. The vendor function
/// continues with software event/list initialization, and the outer
/// controller-init path still requires LP, BLE-stack and HCI initialization.
/// Consequently this state exposes neither common-PHY enable nor task, IRQ,
/// HCI, controller or Link-Layer readiness. Dropping it is fail-stop because
/// no verified rollback exists after the scheduler MMIO mutation.
#[must_use = "the scheduler-prefix state retains every powered Bluetooth owner"]
pub struct BluetoothSchedulerHardwareListHeadsCleared<P> {
    task: BluetoothTaskResources,
    interrupts: BluetoothInterruptBankOwner,
    platform: BluetoothTeardownPendingPlatform<P>,
    time_scale: BluetoothControllerTimeScale,
}

/// Scheduler-prefix hardware ownership paired with one pristine static
/// Controller runtime epoch.
///
/// This state replaces the vendor task/event container but still exposes no
/// PHY, BTBB, IRQ, HCI or Link-Layer readiness. The open scheduler item queue,
/// remaining hardware initialization and stable ISR publication are missing.
#[must_use = "the bound runtime retains every powered Bluetooth owner"]
pub struct BluetoothSchedulerRuntimeResourcesBound<P, const MODEM_TIMER_CAPACITY: usize> {
    _task: BluetoothTaskResources,
    _interrupts: BluetoothInterruptBankOwner,
    _platform: BluetoothTeardownPendingPlatform<P>,
    time_scale: BluetoothControllerTimeScale,
    runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
}

impl<P> BluetoothSchedulerHardwareListHeadsCleared<P> {
    /// Return the scheduler scale established by the preceding HAL component.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Bind one freshly constructed no-RTOS runtime aggregate to this exact
    /// powered hardware epoch.
    ///
    /// The operation performs no MMIO. Resources are consumed by value so
    /// workers, queues and event cells from another epoch cannot be paired
    /// with the retained task/interrupt partitions.
    pub fn bind_runtime_resources<const MODEM_TIMER_CAPACITY: usize>(
        self,
        runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
    ) -> BluetoothSchedulerRuntimeResourcesBound<P, MODEM_TIMER_CAPACITY> {
        assert!(
            runtime.is_pristine(),
            "only a pristine Controller runtime can enter a hardware epoch"
        );
        BluetoothSchedulerRuntimeResourcesBound {
            _task: self.task,
            _interrupts: self.interrupts,
            _platform: self.platform,
            time_scale: self.time_scale,
            runtime,
        }
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize>
    BluetoothSchedulerRuntimeResourcesBound<P, MODEM_TIMER_CAPACITY>
{
    /// Number of fixed modem timer slots retained by the bound epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.runtime.modem_timer_capacity()
    }

    /// Return the scheduler scale retained by this exact hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Whether no software event has entered the bound epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.runtime.is_pristine()
    }

    /// Borrow the matching interrupt and task runtime endpoints from this
    /// hardware-bound epoch.
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
    /// Remove all sixteen scheduler hardware-list heads in positional order.
    ///
    /// This consumes the completed controller HAL state before the first
    /// scheduler-table write. The result intentionally has no next hardware
    /// transition until the remaining scheduler lifecycle has independent
    /// evidence and production owners.
    #[cfg(target_arch = "riscv32")]
    pub fn clear_scheduler_hardware_list_heads(
        self,
    ) -> BluetoothSchedulerHardwareListHeadsCleared<P> {
        self.clear_scheduler_hardware_list_heads_with(|task| {
            task.clear_scheduler_hardware_list_heads();
        })
    }

    fn clear_scheduler_hardware_list_heads_with(
        self,
        clear: impl FnOnce(&mut BluetoothTaskResources),
    ) -> BluetoothSchedulerHardwareListHeadsCleared<P> {
        let Self {
            mut task,
            interrupts,
            platform,
            time_scale,
        } = self;
        clear(&mut task);
        BluetoothSchedulerHardwareListHeadsCleared {
            task,
            interrupts,
            platform,
            time_scale,
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
    fn controller_hal_precedes_scheduler_prefix_and_arms_fail_stop() {
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
        let scheduler = initialized.clear_scheduler_hardware_list_heads_with(|_| {
            scheduler_operations.borrow_mut().push("scheduler-prefix");
        });
        assert_eq!(
            operations.borrow().as_slice(),
            ["controller-hal", "scheduler-prefix"]
        );
        assert_eq!(scheduler.controller_time_scale(), time_scale);
        let mut bound =
            scheduler.bind_runtime_resources(BluetoothControllerRuntimeResources::<4>::new());
        assert_eq!(bound.modem_timer_capacity(), 4);
        assert_eq!(bound.controller_time_scale(), time_scale);
        assert!(bound.runtime_is_pristine());
        let (interrupt, task) = bound.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        drop((interrupt, task));
        drop(bound);
        assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
    }
}
