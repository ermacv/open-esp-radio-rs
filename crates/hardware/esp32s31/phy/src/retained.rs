//! Registered PHY retained between protocol routes with RF closed.
//!
//! A [`RetainedPhy`] couples the HAL's retained radio root to the registered
//! PHY domain whose RF-close graph completed on the previous route. It is the
//! only owner through which a protocol switch keeps the registration and
//! calibration epoch: the next route enters from it and wakes RF with the
//! vendor retained-wake graph instead of powering and registering the PHY
//! again. The client set is empty and no RF client may run while it exists.

use oer_esp32s31_hal::root::RetainedRadioHardware;

use crate::{PhyState, RegisteredBluetoothPhyRfClosed, registered_route::PhyDomain};

/// Powered, registered PHY with RF closed, held between protocol routes.
///
/// The owner is unique and has no public constructor, so a domain cannot be
/// paired with a radio root of another registration:
///
/// ```compile_fail
/// use oer_esp32s31_phy::RetainedPhy;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<RetainedPhy>();
/// ```
#[must_use = "dropping the retained PHY permanently loses the powered radio"]
pub struct RetainedPhy {
    hardware: RetainedRadioHardware,
    domain: PhyDomain,
}

impl RetainedPhy {
    pub(crate) fn from_parts(hardware: RetainedRadioHardware, domain: PhyDomain) -> Self {
        debug_assert!(domain.client_snapshot().is_empty());
        Self { hardware, domain }
    }

    /// Borrow the retained registered calibration state.
    pub const fn state(&self) -> &PhyState {
        self.domain.phy_state()
    }

    /// Borrow the retained registered PHY domain.
    pub const fn domain(&self) -> &PhyDomain {
        &self.domain
    }

    /// Couple a closed Bluetooth domain to the root its Controller released.
    ///
    /// # Errors
    ///
    /// Returns both owners unchanged when `hardware` is not described by the
    /// closed domain's registration.
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns both affine owners without allocation"
    )]
    pub fn from_bluetooth(
        hardware: RetainedRadioHardware,
        closed: RegisteredBluetoothPhyRfClosed,
    ) -> Result<Self, RetainedPhyMismatch> {
        if closed
            .domain
            .clients
            .describes_epoch(hardware.registration_epoch())
        {
            Ok(Self::from_parts(hardware, closed.domain))
        } else {
            Err(RetainedPhyMismatch { hardware, closed })
        }
    }

    /// Hand the retained root to a Bluetooth Controller boot together with
    /// the closed domain it must wake on the Bluetooth route.
    pub fn into_bluetooth(self) -> (RetainedRadioHardware, RegisteredBluetoothPhyRfClosed) {
        (
            self.hardware,
            RegisteredBluetoothPhyRfClosed {
                domain: self.domain,
            },
        )
    }

    /// Enter the Wi-Fi route. The returned owner still has RF closed; wake it
    /// with [`crate::RegisteredPhyRfClosed::wake_rf`].
    pub fn into_wifi<P>(self, peripheral: P) -> crate::RegisteredPhyRfClosed<P> {
        crate::RegisteredPhyRfClosed::from_retained(
            oer_esp32s31_hal::owner::Radio::from_retained(peripheral, self.hardware),
            self.domain,
        )
    }
}

/// A closed Bluetooth domain presented with a root of another registration.
#[must_use = "a rejected pairing still owns the radio root and the domain"]
pub struct RetainedPhyMismatch {
    hardware: RetainedRadioHardware,
    closed: RegisteredBluetoothPhyRfClosed,
}

impl RetainedPhyMismatch {
    /// Recover both unchanged owners.
    pub fn into_parts(self) -> (RetainedRadioHardware, RegisteredBluetoothPhyRfClosed) {
        (self.hardware, self.closed)
    }
}
