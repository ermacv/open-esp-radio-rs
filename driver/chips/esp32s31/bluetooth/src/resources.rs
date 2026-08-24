//! Lossless ownership transitions for standalone Bluetooth hardware.

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_hal::{
    BluetoothSharedPhyBorrow, SharedPhyHal, WifiBasebandEnableObservation,
};
use open_esp_radio_esp32s31_pac::{
    BluetoothColdRegisters as PacBluetoothColdRegisters, RadioHardware,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_pac::{
    BluetoothInterruptSetup as PacBluetoothInterruptSetup,
    BluetoothTaskRegisters as PacBluetoothTaskRegisters,
};

/// Exclusive standalone Bluetooth ownership before any lifecycle transaction.
///
/// Construction consumes the protocol-neutral radio root. Releasing this
/// value returns that exact root, so a later Wi-Fi or coexistence lifecycle
/// does not need to steal or manufacture another MMIO owner.
#[must_use = "Bluetooth physical resources retain the unique radio owner"]
pub struct BluetoothPhysicalResources {
    registers: PacBluetoothColdRegisters,
}

impl BluetoothPhysicalResources {
    /// Enter the exclusive Bluetooth ownership route without touching MMIO.
    pub fn from_radio_hardware(hardware: RadioHardware) -> Self {
        Self {
            registers: hardware.into_bluetooth(),
        }
    }

    /// Return every physical radio owner to the protocol-neutral root.
    ///
    /// This is valid only before a future powered lifecycle starts, or after
    /// that lifecycle has completed its shutdown and rollback transaction.
    pub fn release(self) -> RadioHardware {
        self.registers.release()
    }

    /// Separate task-side controller ownership from the inactive IRQ bank.
    ///
    /// The transition performs no MMIO. In particular it does not configure
    /// controller masks or a CPU interrupt route.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn separate_interrupt_owner(
        self,
    ) -> (BluetoothTaskResources, BluetoothInterruptBankOwner) {
        let (task, interrupts) = self.registers.separate_interrupt_owner();
        (
            BluetoothTaskResources { registers: task },
            BluetoothInterruptBankOwner {
                registers: interrupts,
            },
        )
    }
}

/// Ordinary task-side owner of the standalone Bluetooth controller region.
///
/// No MMIO operation is exposed until its finite lifecycle transaction has
/// independent vendor evidence.
#[must_use = "the Bluetooth task owner must be reunited before release"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothTaskResources {
    registers: PacBluetoothTaskRegisters,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothTaskResources {
    /// Borrow the protocol-neutral radio PHY for one finite lower-layer scope.
    ///
    /// The caller supplies an explicit lifecycle-owned Wi-Fi baseband
    /// readback. Selecting the Bluetooth route alone is not treated as proof
    /// that the shared settle condition is false.
    pub(crate) fn shared_phy_hal(
        &mut self,
        wifi_baseband: WifiBasebandEnableObservation,
    ) -> SharedPhyHal<'_> {
        self.registers.borrow_shared_phy(wifi_baseband)
    }

    /// Reunite a quiescent task owner with its inactive interrupt owner.
    pub(crate) fn reunite(
        self,
        interrupts: BluetoothInterruptBankOwner,
    ) -> BluetoothPhysicalResources {
        BluetoothPhysicalResources {
            registers: self.registers.into_cold(interrupts.registers),
        }
    }
}

/// Inactive owner of the Bluetooth controller interrupt bank.
#[must_use = "the interrupt owner must be installed or reunited"]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothInterruptBankOwner {
    registers: PacBluetoothInterruptSetup,
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::{SharedPhyAccess, WifiBasebandEnableObservation};
    use open_esp_radio_esp32s31_pac::RadioHardware;

    use super::BluetoothPhysicalResources;

    #[test]
    fn task_and_interrupt_owners_reunite_into_the_same_radio_root() {
        let resources =
            BluetoothPhysicalResources::from_radio_hardware(RadioHardware::for_validation());
        let (task, setup) = resources.separate_interrupt_owner();
        let hardware = task.reunite(setup).release();

        // Re-entering Wi-Fi proves that every inactive protocol and shared
        // owner survived the complete Bluetooth ownership roundtrip.
        let _wifi = hardware.into_wifi();
    }

    #[test]
    fn shared_phy_is_a_borrow_not_an_independent_owner() {
        fn accepts_shared_phy(_: &mut impl SharedPhyAccess) {}

        let resources =
            BluetoothPhysicalResources::from_radio_hardware(RadioHardware::for_validation());
        let (mut task, setup) = resources.separate_interrupt_owner();
        {
            let mut phy =
                task.shared_phy_hal(WifiBasebandEnableObservation::from_platform_readback(false));
            accepts_shared_phy(&mut phy);
        }

        // Reuniting after the borrow ends proves that the PHY partition never
        // became an independently releasable capability.
        let _hardware = task.reunite(setup).release();
    }
}
