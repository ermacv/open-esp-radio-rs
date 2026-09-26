//! Wi-Fi as a client of the shared PHY domain.
//!
//! The HAL chain holds the Wi-Fi partition; the registered PHY domain lives
//! under the arbiter as [`ConcurrentPhy`]. After its module clocks, ESP-IDF's
//! Wi-Fi start runs `esp_phy_enable(PHY_MODEM_WIFI)`. [`join_wifi`] records
//! that on the [`WifiClocked`] owner and issues the affine
//! [`WifiPhyMembership`]; [`leave_wifi`] consumes it again. Wi-Fi uses no
//! BTBB baseband.

use core::fmt;

use oer_esp32s31_hal::{ieee80211::client::WifiClocked, shared_radio::SharedRadioLease};

use crate::{
    concurrent::{
        ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError, acquire_client, release_client,
    },
    state::client::{PhyModemClient, PhyPllTrackClock},
};

/// Wi-Fi holds a client bit in the shared PHY domain.
///
/// ```compile_fail
/// use oer_esp32s31_phy::wifi_client::WifiPhyMembership;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<WifiPhyMembership>();
/// ```
#[must_use = "the Wi-Fi PHY membership must be left through leave_wifi"]
pub struct WifiPhyMembership {
    _private: (),
}

impl fmt::Debug for WifiPhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WifiPhyMembership")
    }
}

/// Enter Wi-Fi into the shared PHY domain: `esp_phy_enable(PHY_MODEM_WIFI)`.
///
/// `clocked` proves the Wi-Fi module clocks. The returned
/// [`ConcurrentAcquire::TrackingDue`] means the domain must run its tracking
/// before the MAC may use RF.
///
/// # Errors
///
/// The domain is not registered and settled, or the client set rejects the
/// client; nothing changed.
pub fn join_wifi(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &WifiClocked,
    clock: &mut impl PhyPllTrackClock,
) -> Result<(WifiPhyMembership, ConcurrentAcquire), ConcurrentPhyError> {
    let _ = clocked;
    let acquired = acquire_client(lease, PhyModemClient::Wifi, clock)?;
    Ok((WifiPhyMembership { _private: () }, acquired))
}

/// Failed leave retaining the membership.
#[must_use = "a failed leave still holds the Wi-Fi PHY membership"]
pub struct WifiPhyLeaveFailure {
    membership: WifiPhyMembership,
    error: ConcurrentPhyError,
}

impl WifiPhyLeaveFailure {
    pub const fn error(&self) -> ConcurrentPhyError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> WifiPhyMembership {
        self.membership
    }
}

impl fmt::Debug for WifiPhyLeaveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiPhyLeaveFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Leave the shared PHY domain: `esp_phy_disable(PHY_MODEM_WIFI)`. Returns
/// whether Wi-Fi was the last PHY client; the caller then closes RF through
/// the radio system. No register access happens here.
///
/// # Errors
///
/// The domain rejects the release (tracking pending, poisoned); nothing
/// changed and the membership is returned.
pub fn leave_wifi(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &WifiClocked,
    membership: WifiPhyMembership,
) -> Result<bool, WifiPhyLeaveFailure> {
    let _ = clocked;
    match release_client(lease, PhyModemClient::Wifi) {
        Ok(last) => {
            let WifiPhyMembership { _private: () } = membership;
            Ok(last)
        }
        Err(error) => Err(WifiPhyLeaveFailure { membership, error }),
    }
}

#[cfg(test)]
mod tests;
