//! Narrow borrowed HAL capability for ESP32-S31 Bluetooth controller MMIO.
//!
//! The Bluetooth lifecycle retains the unique PAC task owner. Lower layers
//! receive only this finite borrow and named operations; they cannot recover,
//! move or duplicate the underlying register partition.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_pac::BluetoothTaskRegisters;

/// Exclusive finite borrow of the Bluetooth controller task-side registers.
///
/// This type deliberately exposes neither `Deref`, a raw PAC accessor nor a
/// constructor. New operations belong here only after their PAC transaction
/// and lifecycle prerequisites are independently bounded.
pub struct BluetoothControllerHal<'registers> {
    registers: &'registers mut BluetoothTaskRegisters,
}

impl BluetoothControllerHal<'_> {
    /// Clear the low twenty state bits of all sixteen scheduler entries.
    ///
    /// This is only the reviewed controller-initialization prefix. It does not
    /// establish scheduler, Link Layer or HCI readiness. The borrow proves
    /// exclusive register access, not powered lifecycle state; the caller must
    /// retain the independently established clock/reset prerequisite.
    pub fn clear_scheduler_table_low_bits(&mut self) {
        self.registers.clear_scheduler_table_low_bits();
    }
}

mod sealed {
    use open_esp_radio_esp32s31_pac::BluetoothTaskRegisters;

    pub trait BluetoothControllerHalBorrow {
        fn bluetooth_task_registers_mut(&mut self) -> &mut BluetoothTaskRegisters;
    }

    impl BluetoothControllerHalBorrow for BluetoothTaskRegisters {
        fn bluetooth_task_registers_mut(&mut self) -> &mut BluetoothTaskRegisters {
            self
        }
    }
}

/// Sealed conversion from the exclusive PAC task owner to one finite
/// controller HAL borrow.
///
/// This conversion proves aliasing only. It deliberately does not manufacture
/// a powered-controller typestate; production operations remain sequenced by
/// the Bluetooth lifecycle owner above this borrow.
///
/// The borrow follows ordinary Rust exclusivity. For example, two simultaneous
/// controller borrows cannot be created:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_hal::BluetoothControllerHalBorrow;
///
/// fn duplicate(owner: &mut impl BluetoothControllerHalBorrow) {
///     let first = owner.borrow_bluetooth_controller();
///     let second = owner.borrow_bluetooth_controller();
///     let _ = (first, second);
/// }
/// ```
#[doc(hidden)]
pub trait BluetoothControllerHalBorrow: sealed::BluetoothControllerHalBorrow {
    /// Borrow the controller registers without exposing their PAC owner.
    fn borrow_bluetooth_controller(&mut self) -> BluetoothControllerHal<'_> {
        BluetoothControllerHal {
            registers: sealed::BluetoothControllerHalBorrow::bluetooth_task_registers_mut(self),
        }
    }
}

impl BluetoothControllerHalBorrow for BluetoothTaskRegisters {}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use super::BluetoothControllerHalBorrow;

    #[test]
    fn controller_borrow_ends_before_cold_owner_reconstruction() {
        let cold = RadioHardware::for_validation().into_bluetooth();
        let (mut task, interrupts) = cold.separate_interrupt_owner();
        {
            let _controller = task.borrow_bluetooth_controller();
        }
        let hardware = task.into_cold(interrupts).release();

        // Re-entering Wi-Fi proves that the finite HAL borrow neither moved nor
        // duplicated any protocol-neutral owner.
        let _wifi = hardware.into_wifi();
    }
}
