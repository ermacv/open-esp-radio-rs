//! Cold Wi-Fi route before the task/ISR split.

use oer_esp32s31_pac::{
    CoexistenceLowPowerClockObservation, MacInterruptSetup as PacMacInterruptSetup,
    RadioPhyRegisters, WifiRadioRegisters,
};

use crate::{
    clock::WifiClocks,
    ieee80211::station_wake::StationWakeState,
    phy::restore::PhyRouteState,
    power::RoutePower,
    root::{RadioHardware, RadioPhyReleaseError, RetainedBluetooth, WifiRoute},
    types::{MacInterruptEnableState, MacInterruptMask},
};

/// Route-scoped Wi-Fi state that travels between the cold and runtime owners.
pub(crate) struct WifiRouteState {
    retained: RetainedBluetooth,
    clocks: WifiClocks,
    phy_state: PhyRouteState,
    station_wake: StationWakeState,
}

impl WifiRouteState {
    pub(crate) fn phy_state_mut(&mut self) -> &mut PhyRouteState {
        &mut self.phy_state
    }

    pub(crate) fn station_wake_mut(&mut self) -> &mut StationWakeState {
        &mut self.station_wake
    }
}

/// Pre-runtime Wi-Fi route that still controls the cold MAC interrupt fields.
///
/// PHY setup, cold MAC initialization and polling-only scan/authentication use
/// this owner. [`Self::into_running`] permanently moves the MAC and WDEVPWR
/// interrupt banks to the runtime setup token; a closed ISR epoch can return
/// the same ownership through [`Self::from_running`].
pub(crate) struct WifiColdRegisters {
    registers: WifiRadioRegisters,
    interrupts: PacMacInterruptSetup,
    route: WifiRouteState,
}

impl WifiColdRegisters {
    /// Consume the neutral root into the exclusive Wi-Fi route.
    pub(crate) fn from_hardware(hardware: RadioHardware) -> Self {
        let WifiRoute {
            registers,
            interrupts,
            phy,
            retained,
        } = hardware.into_wifi();
        Self {
            registers,
            interrupts,
            route: WifiRouteState {
                retained,
                clocks: WifiClocks::default(),
                phy_state: phy,
                station_wake: StationWakeState::default(),
            },
        }
    }

    /// Complete the one-way cold-to-running ownership transition.
    ///
    /// This operation performs no MMIO. The returned setup token keeps MAC
    /// interrupts masked until its consuming activation transaction.
    pub(crate) fn into_running(self) -> (WifiRadioRegisters, PacMacInterruptSetup, WifiRouteState) {
        (self.registers, self.interrupts, self.route)
    }

    /// Reunite a quiescent runtime register set with its inactive interrupt
    /// setup. The caller must first disable the CPU routes and recover the
    /// setup from the finite ISR epoch. This conversion performs no MMIO.
    pub(crate) fn from_running(
        registers: WifiRadioRegisters,
        interrupts: PacMacInterruptSetup,
        route: WifiRouteState,
    ) -> Self {
        Self {
            registers,
            interrupts,
            route,
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
        if let Err(error) = crate::root::check_phy_restore_complete(&self.route.phy_state) {
            return Err((self, error));
        }
        let phy = self.registers.radio_phy_mut();
        self.route.clocks.shared.release_all(phy);
        if let Err(checkpoint) = self.route.clocks.power.restore(phy, false) {
            return Err((self, RadioPhyReleaseError::WifiPowerRestore(checkpoint)));
        }
        Ok(RadioHardware::from_wifi(
            self.registers,
            self.interrupts,
            self.route.phy_state,
            self.route.retained,
        ))
    }

    /// Capture the reversible Wi-Fi power baseline before the first mutation.
    pub(crate) fn prepare_wifi_power_epoch(&mut self) {
        self.route.clocks.power.prepare(self.registers.radio_phy());
    }

    /// Borrow the shared PHY and route clock leases for the power sequence.
    pub(crate) fn power_route(&mut self) -> RoutePower<'_> {
        RoutePower {
            phy: self.registers.radio_phy_mut(),
            leases: &mut self.route.clocks.shared,
        }
    }

    /// Retain the coexistence clock once for the Wi-Fi MAC epoch.
    pub(crate) fn retain_coexistence_clock(&mut self) {
        self.route
            .clocks
            .shared
            .retain_coexistence(self.registers.radio_phy_mut());
    }

    /// The route PHY software state.
    pub(crate) fn phy_state(&self) -> &PhyRouteState {
        &self.route.phy_state
    }

    /// Borrow the shared PHY together with the route restore slot.
    pub(crate) fn phy_parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.registers.radio_phy_mut(), &mut self.route.phy_state)
    }

    /// Borrow the Wi-Fi register set together with the route restore slot.
    pub(crate) fn radio_parts_mut(&mut self) -> (&mut WifiRadioRegisters, &mut PhyRouteState) {
        (&mut self.registers, &mut self.route.phy_state)
    }

    #[cfg(test)]
    pub(crate) fn phy_state_mut(&mut self) -> &mut PhyRouteState {
        &mut self.route.phy_state
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
