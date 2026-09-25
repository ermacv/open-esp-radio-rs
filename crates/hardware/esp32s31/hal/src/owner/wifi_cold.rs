//! Cold Wi-Fi route before the task/ISR split.

use oer_esp32s31_pac::{
    CoexistenceLowPowerClockObservation, MacInterruptSetup as PacMacInterruptSetup,
    WifiRadioRegisters,
};

use crate::{
    root::{RadioHardware, RadioPhyReleaseError, RetainedBluetooth, WifiRoute},
    types::{MacInterruptEnableState, MacInterruptMask},
};

/// Pre-runtime Wi-Fi route that still controls the cold MAC interrupt fields.
///
/// PHY setup, cold MAC initialization and polling-only scan/authentication use
/// this owner. [`Self::into_running`] permanently moves the MAC and WDEVPWR
/// interrupt banks to the runtime setup token; a closed ISR epoch can return
/// the same ownership through [`Self::from_running`].
pub(crate) struct WifiColdRegisters {
    registers: WifiRadioRegisters,
    interrupts: PacMacInterruptSetup,
    retained: RetainedBluetooth,
}

impl WifiColdRegisters {
    /// Consume the neutral root into the exclusive Wi-Fi route.
    pub(crate) fn from_hardware(hardware: RadioHardware) -> Self {
        let WifiRoute {
            registers,
            interrupts,
            retained,
        } = hardware.into_wifi();
        Self {
            registers,
            interrupts,
            retained,
        }
    }

    /// Complete the one-way cold-to-running ownership transition.
    ///
    /// This operation performs no MMIO. The returned setup token keeps MAC
    /// interrupts masked until its consuming activation transaction.
    pub(crate) fn into_running(
        self,
    ) -> (WifiRadioRegisters, PacMacInterruptSetup, RetainedBluetooth) {
        (self.registers, self.interrupts, self.retained)
    }

    /// Reunite a quiescent runtime register set with its inactive interrupt
    /// setup. The caller must first disable the CPU routes and recover the
    /// setup from the finite ISR epoch. This conversion performs no MMIO.
    pub(crate) fn from_running(
        registers: WifiRadioRegisters,
        interrupts: PacMacInterruptSetup,
        retained: RetainedBluetooth,
    ) -> Self {
        Self {
            registers,
            interrupts,
            retained,
        }
    }

    /// Return every Wi-Fi, Bluetooth and shared owner to the neutral root.
    ///
    /// The PHY layer remains responsible for closing RF and analog state before
    /// this call. Release drops retained shared-clock leases, restores the
    /// non-monotonic clock/reset fields captured before Wi-Fi power-up, and
    /// only then reconstructs the protocol-neutral owner. Global modem ICG
    /// maps remain installed as monotonic platform initialization.
    ///
    /// # Errors
    ///
    /// Returns this owner while TX-DC PWDET, TX-IQ, RX-DCO, or Bluetooth
    /// TX-power control still awaits restoration, or when a cold-power
    /// baseline fails readback.
    pub(crate) fn release(mut self) -> Result<RadioHardware, (Self, RadioPhyReleaseError)> {
        if let Err(error) = crate::root::check_phy_restore_complete(self.registers.radio_phy()) {
            return Err((self, error));
        }
        self.registers.release_retained_shared_clocks();
        if let Err(checkpoint) = self.registers.radio_phy_mut().restore_wifi_power_epoch() {
            return Err((self, RadioPhyReleaseError::WifiPowerRestore(checkpoint)));
        }
        Ok(RadioHardware::from_wifi(
            self.registers,
            self.interrupts,
            self.retained,
        ))
    }

    /// Capture the reversible Wi-Fi power baseline before the first mutation.
    pub(crate) fn prepare_wifi_power_epoch(&mut self) {
        self.registers.radio_phy_mut().prepare_wifi_power_epoch();
    }

    pub(crate) fn radio(&self) -> &WifiRadioRegisters {
        &self.registers
    }

    pub(crate) fn radio_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.registers
    }

    /// Read the cold initializer's currently published interrupt mask.
    pub(crate) fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        self.interrupts.mac_interrupt_enable()
    }

    /// Mask every MAC event and acknowledge every stale cold event.
    pub(crate) fn mask_and_clear_all_mac_interrupts(&mut self) {
        self.interrupts.mask_and_clear_all_mac_interrupts();
    }

    pub(crate) fn request_mac_cold_start(&mut self) {
        self.registers.request_mac_cold_start();
    }

    pub(crate) fn sample_mac_cold_start_ready(&self) -> bool {
        self.registers.sample_mac_cold_start_ready()
    }

    pub(crate) fn mask_all_mac_interrupts(&mut self) {
        self.interrupts.mask_all_mac_interrupts();
    }

    pub(crate) fn clear_all_mac_interrupts(&mut self) {
        self.interrupts.clear_all_mac_interrupts();
    }

    pub(crate) fn enable_mac_with_interrupt_mask(&mut self, event_mask: MacInterruptMask) {
        self.registers
            .enable_mac_with_interrupt_mask(&mut self.interrupts, event_mask);
    }

    pub(crate) fn initialize_mac_hal_tail(
        &mut self,
        event_mask: MacInterruptMask,
        slow_clock_calibration: u32,
    ) -> bool {
        self.registers.initialize_mac_hal_tail(
            &mut self.interrupts,
            event_mask,
            slow_clock_calibration,
        )
    }

    pub(crate) fn enable_wifi_mac_clocks(&mut self) {
        self.registers.radio_phy_mut().enable_wifi_mac_clocks();
    }

    pub(crate) fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.registers.radio_phy_mut().set_wifi_mac_reset(asserted);
    }

    pub(crate) fn configure_modem_source_clocks(&mut self) {
        self.registers
            .radio_phy_mut()
            .configure_modem_source_clocks();
    }

    pub(crate) fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        self.registers.sample_coexistence_low_power_clock()
    }
}
