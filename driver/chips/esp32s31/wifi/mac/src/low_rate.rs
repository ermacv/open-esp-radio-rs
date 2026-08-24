//! Cross-layer ownership boundary for the PHY low-rate path.

use open_esp_radio_esp32s31_hal::wifi_mac::{WifiMacColdHal, WifiMacHal};

/// Narrow PHY capability needed by the MAC cold-start policy.
///
/// The MAC chooses whether low-rate operation is wanted, while the generated
/// PAC owns the PHY register identities and the ordered hardware edges.
pub trait MacLowRateHardware {
    fn disable_phy_low_rate(&mut self);
}

impl MacLowRateHardware for WifiMacColdHal<'_> {
    fn disable_phy_low_rate(&mut self) {
        WifiMacColdHal::disable_phy_low_rate(self);
    }
}

/// Runtime authority for the complete reviewed PHY low-rate gate.
///
/// This deliberately says nothing about LR PLCP encoding, rate selection or
/// interoperability. It only owns the exact three register edges and the ROM
/// status readback.
pub trait MacRuntimeLowRateHardware {
    fn phy_low_rate_enabled(&self) -> bool;

    fn configure_phy_low_rate(&mut self, enabled: bool);
}

impl MacRuntimeLowRateHardware for WifiMacHal<'_> {
    fn phy_low_rate_enabled(&self) -> bool {
        WifiMacHal::phy_low_rate_enabled(self)
    }

    fn configure_phy_low_rate(&mut self, enabled: bool) {
        WifiMacHal::configure_phy_low_rate(self, enabled);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacLowRateState {
    Disabled,
    Enabled,
}

impl MacLowRateState {
    const fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    const fn enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacLowRateTransition {
    Activate,
    Restore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacLowRateTransitionError {
    pub transition: MacLowRateTransition,
    pub expected: MacLowRateState,
    pub observed: MacLowRateState,
}

/// Exclusive, scoped low-rate activation around one runtime MAC authority.
///
/// The session remembers the entry state instead of assuming cold-start
/// `Disabled`. [`Self::restore`] must succeed before the hardware authority is
/// returned, so stop/error paths cannot accidentally strand the shared PHY in
/// LR mode.
#[must_use = "the low-rate session must be restored to return its hardware owner"]
pub struct MacLowRateSession<'hardware, H: MacRuntimeLowRateHardware> {
    hardware: &'hardware mut H,
    previous: MacLowRateState,
}

impl<'hardware, H: MacRuntimeLowRateHardware> MacLowRateSession<'hardware, H> {
    pub fn activate(hardware: &'hardware mut H) -> Result<Self, MacLowRateTransitionError> {
        let previous = MacLowRateState::from_enabled(hardware.phy_low_rate_enabled());
        hardware.configure_phy_low_rate(true);
        let observed = MacLowRateState::from_enabled(hardware.phy_low_rate_enabled());
        if observed != MacLowRateState::Enabled {
            // The entry state was Disabled whenever activation needed a
            // transition. Reapply it before returning the failed owner.
            hardware.configure_phy_low_rate(previous.enabled());
            return Err(MacLowRateTransitionError {
                transition: MacLowRateTransition::Activate,
                expected: MacLowRateState::Enabled,
                observed,
            });
        }
        Ok(Self { hardware, previous })
    }

    pub const fn previous_state(&self) -> MacLowRateState {
        self.previous
    }

    /// Borrow the same exclusive MAC authority for the bounded operation that
    /// requires the low-rate gate. The session remains the restore owner.
    pub fn hardware_mut(&mut self) -> &mut H {
        self.hardware
    }

    /// Restore the exact entry state and return the runtime hardware owner.
    /// A failed readback returns the complete session so the caller can retry
    /// restoration or escalate to a radio reset without losing authority.
    #[allow(clippy::result_large_err)]
    pub fn restore(self) -> Result<&'hardware mut H, (MacLowRateTransitionError, Self)> {
        self.hardware
            .configure_phy_low_rate(self.previous.enabled());
        let observed = MacLowRateState::from_enabled(self.hardware.phy_low_rate_enabled());
        if observed != self.previous {
            return Err((
                MacLowRateTransitionError {
                    transition: MacLowRateTransition::Restore,
                    expected: self.previous,
                    observed,
                },
                self,
            ));
        }
        Ok(self.hardware)
    }
}
