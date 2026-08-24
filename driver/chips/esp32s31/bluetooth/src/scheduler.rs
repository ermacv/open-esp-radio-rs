//! First fact-bounded controller-init transaction after clock setup.

use crate::{
    BluetoothClockedResources,
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
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
    _task: BluetoothTaskResources,
    _interrupts: BluetoothInterruptBankOwner,
    _platform: BluetoothTeardownPendingPlatform<P>,
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
        let (resources, platform) = self.into_parts();
        // Arm fail-stop ownership before the first scheduler MMIO mutation.
        let platform = BluetoothTeardownPendingPlatform::new(platform);
        let (mut task, interrupts) = resources.separate_interrupt_owner();
        clear(&mut task);
        BluetoothSchedulerTableLowBitsCleared {
            _task: task,
            _interrupts: interrupts,
            _platform: platform,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use open_esp_radio_esp32s31_pac::RadioHardware;

    use crate::{BluetoothClockControl, BluetoothClockState, BluetoothPhysicalResources};

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
        let resources =
            BluetoothPhysicalResources::from_radio_hardware(RadioHardware::for_validation());
        let clocked = match resources.enable_clocks(FakePlatform) {
            Ok(clocked) => clocked,
            Err(_) => panic!("ready fake platform was rejected"),
        };
        let mut called = false;

        let scheduler = clocked.clear_scheduler_table_low_bits_with(|_| called = true);
        assert!(called);
        drop(scheduler);
        assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
    }
}
