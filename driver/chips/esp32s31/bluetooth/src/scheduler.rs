//! First fact-bounded controller-init transaction after clock setup.

use crate::{
    BluetoothClockedResources, BluetoothControllerInterruptRuntime,
    BluetoothControllerRuntimeResources, BluetoothControllerTaskRuntime,
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
        separate_interrupt_owner,
    },
};

/// Bluetooth hardware after only the sixteen scheduler-table low fields were
/// cleared.
///
/// This is a deliberately terminal research frontier. The vendor function
/// continues with software event/list initialization, and the outer
/// controller-init path still requires LP, BLE-stack and HCI initialization.
/// Consequently this state exposes neither common-PHY enable nor task, IRQ,
/// HCI, controller or Link-Layer readiness. Dropping it is fail-stop because
/// no verified rollback exists after the scheduler MMIO mutation.
#[must_use = "the scheduler-prefix state retains every powered Bluetooth owner"]
pub struct BluetoothSchedulerTableLowBitsCleared<P> {
    task: BluetoothTaskResources,
    interrupts: BluetoothInterruptBankOwner,
    platform: BluetoothTeardownPendingPlatform<P>,
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
    runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>,
}

impl<P> BluetoothSchedulerTableLowBitsCleared<P> {
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

impl<P> BluetoothClockedResources<P> {
    /// Clear bits 19:0 of all sixteen scheduler-table entries in ascending
    /// address order.
    ///
    /// This consumes the reversible clocked state before the first scheduler
    /// write. The result intentionally has no next transition until the rest
    /// of controller init has independent evidence and production owners.
    #[cfg(target_arch = "riscv32")]
    pub fn clear_scheduler_table_low_bits(self) -> BluetoothSchedulerTableLowBitsCleared<P> {
        self.clear_scheduler_table_low_bits_with(|task| {
            task.clear_scheduler_table_low_bits();
        })
    }

    fn clear_scheduler_table_low_bits_with(
        self,
        clear: impl FnOnce(&mut BluetoothTaskResources),
    ) -> BluetoothSchedulerTableLowBitsCleared<P> {
        let (registers, platform) = self.into_parts();
        // Arm fail-stop ownership before the first scheduler MMIO mutation.
        let platform = BluetoothTeardownPendingPlatform::new(platform);
        let (mut task, interrupts) = separate_interrupt_owner(registers);
        clear(&mut task);
        BluetoothSchedulerTableLowBitsCleared {
            task,
            interrupts,
            platform,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use open_esp_radio_esp32s31_pac::RadioHardware;

    use crate::{
        BluetoothClockControl, BluetoothClockState, BluetoothControllerRuntimeResources,
        BluetoothStopped,
    };

    static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

    struct FakePlatform;

    impl Drop for FakePlatform {
        fn drop(&mut self) {
            PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl BluetoothClockControl for FakePlatform {
        fn enable_bluetooth_controller_clocks(&mut self) {}
        fn enable_bluetooth_apb_clocks(&mut self) {}
        fn reset_bluetooth_controller_domains(&mut self) {}
        fn select_main_xtal_low_power_clock(&mut self, _divider: u16) {}

        fn bluetooth_clock_state(&mut self) -> BluetoothClockState {
            BluetoothClockState {
                controller_clocks_enabled: true,
                apb_clocks_enabled: true,
                controller_resets_released: true,
                main_xtal_selected: true,
                low_power_divider: 399,
                low_power_timer_enabled: true,
            }
        }

        fn deselect_low_power_clock(&mut self) {}
        fn disable_bluetooth_apb_clocks(&mut self) {}
        fn disable_bluetooth_controller_clocks(&mut self) {}
    }

    #[test]
    fn scheduler_prefix_consumes_reversible_clocks_and_arms_fail_stop() {
        PLATFORM_DROPS.store(0, Ordering::Relaxed);
        let stopped =
            BluetoothStopped::from_hardware(FakePlatform, RadioHardware::for_validation());
        let clocked = match stopped.enable_clocks() {
            Ok(clocked) => clocked,
            Err(_) => panic!("ready fake platform was rejected"),
        };
        let mut called = false;

        let scheduler = clocked.clear_scheduler_table_low_bits_with(|_| called = true);
        assert!(called);
        let mut bound =
            scheduler.bind_runtime_resources(BluetoothControllerRuntimeResources::<4>::new());
        assert_eq!(bound.modem_timer_capacity(), 4);
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
