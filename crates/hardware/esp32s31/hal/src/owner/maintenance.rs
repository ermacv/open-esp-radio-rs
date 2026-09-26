//! Shared-PHY maintenance admission and the stopped Wi-Fi MAC check.
//!
//! Exclusive Bluetooth maintenance transfers the existing physical owner,
//! rather than minting an RF permission from a timer, client snapshot or
//! mutex. Wi-Fi reaches the shared PHY only through the arbiter lease; its
//! stopped-MAC check confirms the final-client shutdown boundary. MAC and
//! walker readbacks check that prerequisite; they do not retire software DMA
//! leases.

use super::{MacInterruptCheckpoint, MacInterruptSetup, RadioRuntimeOwner, SharedPhyHal, route};
use crate::coex::PhyGrantProtect;
use crate::ieee80211::mac::WifiMacHal;

mod sealed {
    pub trait PhyMaintenanceAccess {}
    pub trait InterruptAuthority {}
    impl InterruptAuthority for super::MacInterruptSetup {}
    impl InterruptAuthority for super::MacInterruptCheckpoint {}
}

/// Closed set of complete Wi-Fi interrupt owners. A cold setup and a paused
/// checkpoint retain distinct types across execution and checked restoration.
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::maintenance::InterruptAuthority;
/// struct TimerRequest;
/// impl InterruptAuthority for TimerRequest {}
/// ```
pub trait InterruptAuthority: sealed::InterruptAuthority {}

/// Checked physical admission to shared-PHY maintenance for one protocol.
///
/// A protocol mints its access only after its own hardware quiescence
/// check: Bluetooth through
/// [`InterruptOutputAfterRoutesOwner::try_phy_maintenance`](crate::bluetooth::InterruptOutputAfterRoutesOwner::try_phy_maintenance).
/// The access lends the shared PHY only under its own route, so a PHY
/// operation can require the matching protocol. It is not a coexistence
/// grant: no other protocol may use RF while it is held.
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::{SharedPhyHal, maintenance::PhyMaintenanceAccess, route};
/// struct Forged;
/// impl PhyMaintenanceAccess for Forged {
///     type Route = route::Bluetooth;
///     fn phy_hal(&mut self) -> SharedPhyHal<'_, route::Bluetooth> {
///         unimplemented!()
///     }
///     fn phy_hal_with_grant(
///         &mut self,
///     ) -> (SharedPhyHal<'_, route::Bluetooth>, oer_esp32s31_hal::coex::PhyGrantProtect<'_>) {
///         unimplemented!()
///     }
/// }
/// ```
pub trait PhyMaintenanceAccess: sealed::PhyMaintenanceAccess {
    /// Protocol route whose quiescence admitted this access.
    type Route: route::Route;

    /// Borrow the shared PHY for one step of the admitted operation.
    fn phy_hal(&mut self) -> SharedPhyHal<'_, Self::Route>;

    /// Borrow the shared PHY together with this owner's PHY grant-protect
    /// request, for maintenance that brackets its hardware regions with it.
    ///
    /// The exclusive route has no radio arbiter; the request carries the
    /// arbiter's cold event-48 priority.
    fn phy_hal_with_grant(&mut self) -> (SharedPhyHal<'_, Self::Route>, PhyGrantProtect<'_>);
}

impl sealed::PhyMaintenanceAccess for crate::bluetooth::BluetoothMaintenanceAccess<'_> {}

impl PhyMaintenanceAccess for crate::bluetooth::BluetoothMaintenanceAccess<'_> {
    type Route = route::Bluetooth;

    fn phy_hal(&mut self) -> SharedPhyHal<'_, route::Bluetooth> {
        crate::owner::SharedPhyBorrow::borrow_shared_phy(self.task)
    }

    fn phy_hal_with_grant(&mut self) -> (SharedPhyHal<'_, route::Bluetooth>, PhyGrantProtect<'_>) {
        self.task.shared_phy_hal_with_grant()
    }
}
impl InterruptAuthority for MacInterruptSetup {}
impl InterruptAuthority for MacInterruptCheckpoint {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    MacActive { state: u8 },
    RxWalkerEnabled,
}

/// Original resources retained when admission has made no hardware changes.
pub struct AdmissionFailure<I: InterruptAuthority = MacInterruptSetup> {
    pub error: Error,
    pub registers: RadioRuntimeOwner,
    pub interrupts: I,
}

impl RadioRuntimeOwner {
    /// Verify the stopped physical boundary without acquiring maintenance or
    /// changing hardware. This is the final-client shutdown admission check.
    pub fn try_confirm_phy_stopped<I: InterruptAuthority>(
        self,
        interrupts: I,
    ) -> Result<(Self, I), AdmissionFailure<I>> {
        confirm_stopped(self, interrupts, |registers| {
            check_stopped(&mut registers.wifi_mac_hal())
        })
    }
}

fn confirm_stopped<I: InterruptAuthority>(
    mut registers: RadioRuntimeOwner,
    interrupts: I,
    check: impl FnOnce(&mut RadioRuntimeOwner) -> Result<(), Error>,
) -> Result<(RadioRuntimeOwner, I), AdmissionFailure<I>> {
    if let Err(error) = check(&mut registers) {
        return Err(AdmissionFailure {
            error,
            registers,
            interrupts,
        });
    }
    Ok((registers, interrupts))
}

// Closed readback interface: tests exercise the actual admission/release
// predicate without emulating register encodings or touching host MMIO.
trait Observation {
    fn mac_active(&mut self) -> u8;
    fn rx_walker_enabled(&mut self) -> bool;
}

impl Observation for WifiMacHal<'_> {
    fn mac_active(&mut self) -> u8 {
        self.channel_active_state()
    }
    fn rx_walker_enabled(&mut self) -> bool {
        WifiMacHal::rx_walker_enabled(self)
    }
}

fn check_stopped(hardware: &mut impl Observation) -> Result<(), Error> {
    let state = hardware.mac_active();
    if state != 0 {
        return Err(Error::MacActive { state });
    }
    if hardware.rx_walker_enabled() {
        return Err(Error::RxWalkerEnabled);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
