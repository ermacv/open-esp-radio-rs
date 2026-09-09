//! Shared-PHY access while the physical radio belongs exclusively to Wi-Fi.
//!
//! This transfers the existing physical owner, rather than minting an RF
//! permission from a timer, client snapshot or mutex. The caller must first
//! drain active TX and stop or pause its RX descriptor epoch. MAC and walker
//! readbacks check that prerequisite; they do not retire software DMA leases.
//! No BLE/IEEE 802.15.4 timeslot or concurrent coexistence grant is implied.

use super::{MacInterruptCheckpoint, MacInterruptSetup, RadioRuntimeOwner, SharedPhyHal};
use crate::ieee80211::mac::WifiMacHal;

mod sealed {
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

/// Non-cloneable physical access retained until PHY execution and restoration
/// finish. Pass this value **by ownership** across the maintenance future.
/// Returning a `SharedPhyHal` borrow alone would leave a reusable radio owner
/// outside a cancelled hardware operation.
///
/// The complete IRQ authority is retained here, so the runtime cannot install its route
/// while the operation owns the registers. There is no arena publication or
/// register-owner extraction except checked release.
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::maintenance::WifiAccess;
/// fn duplicate(access: WifiAccess) -> (WifiAccess, WifiAccess) {
///     (access, access)
/// }
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::{MacInterruptSetup, RadioRuntimeOwner};
/// fn reuse(mut registers: RadioRuntimeOwner, setup: MacInterruptSetup) {
///     let access = registers.try_into_phy_maintenance(setup);
///     let mac = registers.wifi_mac_hal();
/// }
/// ```
/// A paused route cannot be released as a cold setup:
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::{MacInterruptCheckpoint, MacInterruptSetup, maintenance::WifiAccess};
/// fn discard_epoch(access: WifiAccess<MacInterruptCheckpoint>) -> MacInterruptSetup {
///     let (_, setup) = access.try_release().ok().unwrap();
///     setup
/// }
/// ```
#[must_use = "physical maintenance access must be restored and explicitly released"]
pub struct WifiAccess<I: InterruptAuthority = MacInterruptSetup> {
    registers: RadioRuntimeOwner,
    interrupts: I,
}

impl RadioRuntimeOwner {
    /// Transfer stopped or paused physical ownership into PHY maintenance. This does
    /// not stop the MAC, DMA or CPU interrupt route on the caller's behalf.
    pub fn try_into_phy_maintenance<I: InterruptAuthority>(
        self,
        interrupts: I,
    ) -> Result<WifiAccess<I>, AdmissionFailure<I>> {
        admit(self, interrupts, |registers| {
            check_stopped(&mut registers.wifi_mac_hal())
        })
    }
}

// These private transfers keep the production check and ownership branches
// together. Tests inject readback outcomes, never replacement radio behavior.
fn admit<I: InterruptAuthority>(
    mut registers: RadioRuntimeOwner,
    interrupts: I,
    check: impl FnOnce(&mut RadioRuntimeOwner) -> Result<(), Error>,
) -> Result<WifiAccess<I>, AdmissionFailure<I>> {
    if let Err(error) = check(&mut registers) {
        return Err(AdmissionFailure {
            error,
            registers,
            interrupts,
        });
    }
    Ok(WifiAccess {
        registers,
        interrupts,
    })
}

impl<I: InterruptAuthority> WifiAccess<I> {
    /// Narrow PHY borrow within the retained physical operation.
    pub fn phy_hal(&mut self) -> SharedPhyHal<'_> {
        SharedPhyHal {
            registers: self.registers.registers.radio_phy_mut(),
        }
    }

    /// Restore the caller's MAC postconditions after PHY children which may
    /// alter baseband enables. Descriptor and IRQ owners remain unavailable.
    pub fn wifi_mac_hal(&mut self) -> WifiMacHal<'_> {
        self.registers.wifi_mac_hal()
    }

    /// Release only after the completed operation has restored stopped MAC
    /// and DMA. A failed readback retains the entire access, requiring reset;
    /// it cannot yield an apparently stopped owner.
    pub fn try_release(self) -> Result<(RadioRuntimeOwner, I), ReleaseFailure<I>> {
        self.release_with(|registers| check_stopped(&mut registers.wifi_mac_hal()))
    }

    fn release_with(
        mut self,
        check: impl FnOnce(&mut RadioRuntimeOwner) -> Result<(), Error>,
    ) -> Result<(RadioRuntimeOwner, I), ReleaseFailure<I>> {
        if let Err(error) = check(&mut self.registers) {
            return Err(ReleaseFailure {
                error,
                _access: self,
            });
        }
        Ok((self.registers, self.interrupts))
    }
}

#[must_use = "failed restoration retains the physical radio and requires reset"]
pub struct ReleaseFailure<I: InterruptAuthority = MacInterruptSetup> {
    pub error: Error,
    _access: WifiAccess<I>,
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
