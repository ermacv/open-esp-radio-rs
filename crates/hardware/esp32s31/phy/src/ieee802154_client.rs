//! IEEE 802.15.4 as a client of the shared PHY domain.
//!
//! The HAL chain holds the IEEE 802.15.4 partition; the registered PHY domain
//! lives under the arbiter as [`ConcurrentPhy`]. Between the module clocks and
//! the MAC reset, ESP-IDF's `esp_ieee802154_enable` runs `esp_phy_enable` and
//! `esp_btbb_enable`, and `ieee802154_mac_init` then overrides the shared
//! transmit-on delay. [`join_ieee802154`] performs those steps on the
//! [`Ieee802154Clocked`] owner and issues the affine
//! [`Ieee802154PhyMembership`]; [`leave_ieee802154`] consumes it again.
//!
//! [`RegisteredIeee802154Operational`] keeps that membership beside the
//! operational MAC owners, so an interrupt-driven MAC epoch cannot exist apart
//! from a PHY client and a BTBB reference.
//!
//! This module holds the crate's single scoped `unsafe` override: the BTBB
//! gain parameter is projected here from the settled registration that
//! describes the lease, never supplied by a caller.

use core::fmt;

use oer_esp32s31_hal::{
    ieee802154::{
        Ieee802154Clocked, Ieee802154FoundationConfigured, Ieee802154FoundationTransitionFailure,
        Ieee802154Operational,
        mac::{Ieee802154InterruptSetupOwner, Ieee802154TaskOwner},
    },
    shared_radio::{BtbbError, RadioClient, SharedRadioLease},
};

use crate::{
    concurrent::{
        ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError, acquire_client, release_client,
    },
    state::client::{PhyModemClient, PhyPllTrackClock},
};

/// IEEE 802.15.4 holds a client bit in the shared PHY domain and a reference
/// on the shared BTBB baseband, with its transmit-on delay applied.
///
/// ```compile_fail
/// use oer_esp32s31_phy::ieee802154_client::Ieee802154PhyMembership;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<Ieee802154PhyMembership>();
/// ```
#[must_use = "the IEEE 802.15.4 PHY membership must be left through leave_ieee802154"]
pub struct Ieee802154PhyMembership {
    _private: (),
}

impl fmt::Debug for Ieee802154PhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ieee802154PhyMembership")
    }
}

/// Why IEEE 802.15.4 could not join or leave the shared PHY and BTBB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PhyClientError {
    /// The PHY domain rejected the operation.
    Phy(ConcurrentPhyError),
    /// The settled registration no longer describes the shared PHY.
    StaleRegistration,
    /// The BTBB reference is not in the state the operation requires.
    Btbb(BtbbError),
}

/// Enter IEEE 802.15.4 into the shared PHY domain and the shared BTBB
/// baseband, then apply its transmit-on delay.
///
/// This is `esp_phy_enable(PHY_MODEM_IEEE802154)` followed by
/// `esp_btbb_enable` and the shared part of `ieee802154_txon_delay_set`. The
/// returned [`ConcurrentAcquire::TrackingDue`] means the domain must run its
/// tracking before the MAC may use RF.
///
/// # Errors
///
/// Every check runs before the client set or BTBB changes: the domain is not
/// registered and settled, its registration no longer describes the lease,
/// IEEE 802.15.4 already holds BTBB, or the client set rejects the client.
pub fn join_ieee802154(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &Ieee802154Clocked,
    clock: &mut impl PhyPllTrackClock,
) -> Result<(Ieee802154PhyMembership, ConcurrentAcquire), Ieee802154PhyClientError> {
    let epoch = lease.registration_epoch();
    let domain = lease
        .attachment()
        .settled()
        .map_err(Ieee802154PhyClientError::Phy)?;
    if !domain.clients.describes_epoch(epoch) {
        return Err(Ieee802154PhyClientError::StaleRegistration);
    }
    let gain_parameter = domain
        .registered
        .state()
        .register_init_parameters()
        .parameter_120;
    if lease.holds_btbb(RadioClient::Ieee802154) {
        return Err(Ieee802154PhyClientError::Btbb(BtbbError::AlreadyAcquired));
    }
    let acquired = acquire_client(lease, PhyModemClient::Ieee802154, clock)
        .map_err(Ieee802154PhyClientError::Phy)?;

    #[allow(
        unsafe_code,
        reason = "the gain byte is projected from the registration that describes the lease"
    )]
    let btbb = {
        // SAFETY: the settled domain was registered by a completed target
        // run whose epoch is the lease's current registration, and
        // `gain_parameter` is the `phy_param` byte at 0x120 projected from
        // that same registered state. `clocked` proves the IEEE 802.15.4
        // module clocks.
        unsafe { clocked.acquire_btbb(lease, gain_parameter) }
    };
    if let Err(error) = btbb {
        // Both rejections were checked above; undo the client bit when the
        // domain still permits it.
        let _ = release_client(lease, PhyModemClient::Ieee802154);
        return Err(Ieee802154PhyClientError::Btbb(error));
    }
    clocked
        .override_tx_on_delay(lease)
        .map_err(Ieee802154PhyClientError::Btbb)?;
    Ok((Ieee802154PhyMembership { _private: () }, acquired))
}

/// Failed leave retaining the membership.
#[must_use = "a failed leave still holds the IEEE 802.15.4 PHY membership"]
pub struct Ieee802154PhyLeaveFailure {
    membership: Ieee802154PhyMembership,
    error: Ieee802154PhyClientError,
}

impl Ieee802154PhyLeaveFailure {
    pub const fn error(&self) -> Ieee802154PhyClientError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> Ieee802154PhyMembership {
        self.membership
    }
}

impl fmt::Debug for Ieee802154PhyLeaveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PhyLeaveFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Leave the shared PHY domain and drop the BTBB reference. Returns whether
/// IEEE 802.15.4 was the last PHY client.
///
/// This is `esp_btbb_disable` and `esp_phy_disable(PHY_MODEM_IEEE802154)`;
/// neither performs register access here.
///
/// # Errors
///
/// The domain rejects the release (tracking pending, poisoned) or the BTBB
/// reference is missing; nothing changed and the membership is returned.
pub fn leave_ieee802154(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &Ieee802154Clocked,
    membership: Ieee802154PhyMembership,
) -> Result<bool, Ieee802154PhyLeaveFailure> {
    if !lease.holds_btbb(RadioClient::Ieee802154) {
        return Err(Ieee802154PhyLeaveFailure {
            membership,
            error: Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    let last = match release_client(lease, PhyModemClient::Ieee802154) {
        Ok(last) => last,
        Err(error) => {
            return Err(Ieee802154PhyLeaveFailure {
                membership,
                error: Ieee802154PhyClientError::Phy(error),
            });
        }
    };
    let Ieee802154PhyMembership { _private: () } = membership;
    // The reference was checked above, so the release cannot be rejected.
    let _ = clocked.release_btbb(lease);
    Ok(last)
}

/// The owners of one operational IEEE 802.15.4 MAC epoch together with its
/// PHY membership.
#[must_use = "the operational owners must return to their registered route"]
pub struct RegisteredIeee802154Operational {
    /// Task-side MAC registers.
    pub task: Ieee802154TaskOwner,
    /// Inactive interrupt ownership for the platform CPU route.
    pub interrupts: Ieee802154InterruptSetupOwner,
    /// The PHY membership retained while the MAC is operational.
    pub route: RegisteredIeee802154OperationalRoute,
}

impl RegisteredIeee802154Operational {
    /// Hand the foundation owners to the operational MAC under `membership`.
    /// This performs no MMIO.
    pub fn new(
        foundation: Ieee802154FoundationConfigured,
        membership: Ieee802154PhyMembership,
    ) -> Self {
        let Ieee802154Operational { task, interrupts } = foundation.into_operational();
        Self {
            task,
            interrupts,
            route: RegisteredIeee802154OperationalRoute { membership },
        }
    }
}

/// PHY membership retained while the MAC is operational.
#[must_use = "the registered operational route must reunite with its owners"]
pub struct RegisteredIeee802154OperationalRoute {
    membership: Ieee802154PhyMembership,
}

impl RegisteredIeee802154OperationalRoute {
    /// Reunite the quiescent operational owners and prove the MAC
    /// foundation again.
    ///
    /// The operational epoch rewrote the MAC PIB, so the return proves the
    /// foundation, not a static policy.
    ///
    /// # Errors
    ///
    /// A foundation field did not read back; the failure keeps the reset
    /// owner and the membership.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the owner and the membership"
    )]
    pub fn into_foundation(
        self,
        task: Ieee802154TaskOwner,
        interrupts: Ieee802154InterruptSetupOwner,
    ) -> Result<
        (Ieee802154FoundationConfigured, Ieee802154PhyMembership),
        RegisteredIeee802154ReturnFailure,
    > {
        match Ieee802154FoundationConfigured::from_operational(Ieee802154Operational {
            task,
            interrupts,
        }) {
            Ok(foundation) => Ok((foundation, self.membership)),
            Err(failure) => Err(RegisteredIeee802154ReturnFailure {
                failure,
                membership: self.membership,
            }),
        }
    }
}

/// Failed foundation proof on return, retaining the reset owner and the
/// membership.
#[must_use = "a failed return still owns the partition and the PHY membership"]
pub struct RegisteredIeee802154ReturnFailure {
    failure: Ieee802154FoundationTransitionFailure,
    membership: Ieee802154PhyMembership,
}

impl RegisteredIeee802154ReturnFailure {
    /// Borrow the foundation readback failure.
    pub const fn failure(&self) -> &Ieee802154FoundationTransitionFailure {
        &self.failure
    }

    /// Recover the reset owner and the membership.
    pub fn into_parts(
        self,
    ) -> (
        Ieee802154FoundationTransitionFailure,
        Ieee802154PhyMembership,
    ) {
        (self.failure, self.membership)
    }
}

impl fmt::Debug for RegisteredIeee802154ReturnFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154ReturnFailure")
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl Ieee802154PhyMembership {
    fn for_test() -> Self {
        Self { _private: () }
    }
}

#[cfg(test)]
mod tests;
