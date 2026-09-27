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
//! When the vendor puts the radio to sleep, `ieee802154_rf_disable` runs only
//! `esp_phy_disable(PHY_MODEM_IEEE802154)`, and `ieee802154_rf_enable` runs
//! `esp_phy_enable` before the next MAC command; the BTBB reference is held
//! from `esp_ieee802154_enable` to `esp_ieee802154_disable`.
//! [`suspend_ieee802154`] and [`resume_ieee802154`] are that pair: the
//! [`Ieee802154PhySuspended`] owner keeps the BTBB reference without a PHY
//! client bit, so the radio system may close RF after the last client left.
//! [`leave_suspended_ieee802154`] drops the reference of a suspended client.
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
/// baseband.
///
/// This is `esp_phy_enable(PHY_MODEM_IEEE802154)` followed by
/// `esp_btbb_enable`; the pinned `ieee802154_txon_delay_set` writes only the
/// MAC's own delays, so the shared transmit-on delay keeps the value of the
/// BTBB initialization. The
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

/// IEEE 802.15.4 holds its BTBB reference but no PHY client bit: the radio
/// is asleep and RF may be closed.
///
/// ```compile_fail
/// use oer_esp32s31_phy::ieee802154_client::Ieee802154PhySuspended;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<Ieee802154PhySuspended>();
/// ```
#[must_use = "a suspended IEEE 802.15.4 client must resume or leave"]
pub struct Ieee802154PhySuspended {
    _private: (),
}

impl fmt::Debug for Ieee802154PhySuspended {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ieee802154PhySuspended")
    }
}

/// Failed suspension retaining the membership.
#[must_use = "a failed suspension still holds the IEEE 802.15.4 PHY membership"]
pub struct Ieee802154PhySuspendFailure {
    membership: Ieee802154PhyMembership,
    error: Ieee802154PhyClientError,
}

impl Ieee802154PhySuspendFailure {
    pub const fn error(&self) -> Ieee802154PhyClientError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> Ieee802154PhyMembership {
        self.membership
    }
}

impl fmt::Debug for Ieee802154PhySuspendFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PhySuspendFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed resumption or leave retaining the suspended client.
#[must_use = "a failed resumption still holds the suspended IEEE 802.15.4 client"]
pub struct Ieee802154PhySuspendedFailure {
    suspended: Ieee802154PhySuspended,
    error: Ieee802154PhyClientError,
}

impl Ieee802154PhySuspendedFailure {
    pub const fn error(&self) -> Ieee802154PhyClientError {
        self.error
    }

    /// Recover the suspended client for a retry.
    pub fn into_suspended(self) -> Ieee802154PhySuspended {
        self.suspended
    }
}

impl fmt::Debug for Ieee802154PhySuspendedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PhySuspendedFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Put IEEE 802.15.4 to sleep: `ieee802154_rf_disable`, which is
/// `esp_phy_disable(PHY_MODEM_IEEE802154)` alone. The BTBB reference stays.
/// Returns the suspended client and whether IEEE 802.15.4 was the last PHY
/// client; the radio system then closes RF. No register access happens
/// here.
///
/// The caller must have stopped the current MAC operation first, as the
/// vendor's `ieee802154_sleep` does.
///
/// # Errors
///
/// The BTBB reference is missing or the domain rejects the release
/// (tracking pending, poisoned); nothing changed and the membership is
/// returned.
pub fn suspend_ieee802154(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    membership: Ieee802154PhyMembership,
) -> Result<(Ieee802154PhySuspended, bool), Ieee802154PhySuspendFailure> {
    if !lease.holds_btbb(RadioClient::Ieee802154) {
        return Err(Ieee802154PhySuspendFailure {
            membership,
            error: Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    match release_client(lease, PhyModemClient::Ieee802154) {
        Ok(last) => {
            let Ieee802154PhyMembership { _private: () } = membership;
            Ok((Ieee802154PhySuspended { _private: () }, last))
        }
        Err(error) => Err(Ieee802154PhySuspendFailure {
            membership,
            error: Ieee802154PhyClientError::Phy(error),
        }),
    }
}

/// Wake IEEE 802.15.4 before its next MAC command: `ieee802154_rf_enable`,
/// which is `esp_phy_enable(PHY_MODEM_IEEE802154)`. The radio system must
/// have woken closed RF first. The returned
/// [`ConcurrentAcquire::TrackingDue`] means the domain must run its tracking
/// before the MAC may use RF, as after [`join_ieee802154`].
///
/// # Errors
///
/// Every check runs before the client set changes: the domain is not
/// registered, settled and RF-open, its registration no longer describes the
/// lease, the BTBB reference is missing, or the client set rejects the
/// client. The suspended client is returned.
pub fn resume_ieee802154(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    suspended: Ieee802154PhySuspended,
    clock: &mut impl PhyPllTrackClock,
) -> Result<(Ieee802154PhyMembership, ConcurrentAcquire), Ieee802154PhySuspendedFailure> {
    let epoch = lease.registration_epoch();
    let checked = match lease.attachment().settled() {
        Ok(domain) if domain.clients.describes_epoch(epoch) => Ok(()),
        Ok(_) => Err(Ieee802154PhyClientError::StaleRegistration),
        Err(error) => Err(Ieee802154PhyClientError::Phy(error)),
    }
    .and_then(|()| {
        if lease.holds_btbb(RadioClient::Ieee802154) {
            Ok(())
        } else {
            Err(Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired))
        }
    });
    if let Err(error) = checked {
        return Err(Ieee802154PhySuspendedFailure { suspended, error });
    }
    match acquire_client(lease, PhyModemClient::Ieee802154, clock) {
        Ok(acquired) => {
            let Ieee802154PhySuspended { _private: () } = suspended;
            Ok((Ieee802154PhyMembership { _private: () }, acquired))
        }
        Err(error) => Err(Ieee802154PhySuspendedFailure {
            suspended,
            error: Ieee802154PhyClientError::Phy(error),
        }),
    }
}

/// Leave while asleep: `esp_btbb_disable` for a client that already left the
/// PHY domain. No register access happens here.
///
/// # Errors
///
/// The BTBB reference is missing; the suspended client is returned.
pub fn leave_suspended_ieee802154(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &Ieee802154Clocked,
    suspended: Ieee802154PhySuspended,
) -> Result<(), Ieee802154PhySuspendedFailure> {
    match clocked.release_btbb(lease) {
        Ok(()) => {
            let Ieee802154PhySuspended { _private: () } = suspended;
            Ok(())
        }
        Err(error) => Err(Ieee802154PhySuspendedFailure {
            suspended,
            error: Ieee802154PhyClientError::Btbb(error),
        }),
    }
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

impl RegisteredIeee802154OperationalRoute {
    /// Put the operational MAC epoch to sleep through
    /// [`suspend_ieee802154`]. Returns the suspended route and whether
    /// IEEE 802.15.4 was the last PHY client.
    ///
    /// # Errors
    ///
    /// As [`suspend_ieee802154`]; the unchanged route is returned.
    pub fn suspend_rf(
        self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    ) -> Result<(RegisteredIeee802154SuspendedRoute, bool), RegisteredIeee802154RouteFailure<Self>>
    {
        match suspend_ieee802154(lease, self.membership) {
            Ok((suspended, last)) => Ok((RegisteredIeee802154SuspendedRoute { suspended }, last)),
            Err(failure) => Err(RegisteredIeee802154RouteFailure {
                error: failure.error(),
                route: Self {
                    membership: failure.into_membership(),
                },
            }),
        }
    }
}

/// The operational MAC epoch while IEEE 802.15.4 is asleep: the BTBB
/// reference is held without a PHY client bit.
#[must_use = "the suspended route must resume or reunite with its owners"]
pub struct RegisteredIeee802154SuspendedRoute {
    suspended: Ieee802154PhySuspended,
}

impl RegisteredIeee802154SuspendedRoute {
    /// Wake the operational MAC epoch through [`resume_ieee802154`].
    ///
    /// # Errors
    ///
    /// As [`resume_ieee802154`]; the unchanged suspended route is returned.
    pub fn resume_rf(
        self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<
        (RegisteredIeee802154OperationalRoute, ConcurrentAcquire),
        RegisteredIeee802154RouteFailure<Self>,
    > {
        match resume_ieee802154(lease, self.suspended, clock) {
            Ok((membership, acquired)) => Ok((
                RegisteredIeee802154OperationalRoute { membership },
                acquired,
            )),
            Err(failure) => Err(RegisteredIeee802154RouteFailure {
                error: failure.error(),
                route: Self {
                    suspended: failure.into_suspended(),
                },
            }),
        }
    }

    /// Reunite the quiescent operational owners while asleep, as
    /// [`RegisteredIeee802154OperationalRoute::into_foundation`] does. The
    /// suspended client then leaves through [`leave_suspended_ieee802154`].
    ///
    /// # Errors
    ///
    /// A foundation field did not read back; the failure keeps the reset
    /// owner and the suspended client.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the owner and the suspended client"
    )]
    pub fn into_foundation(
        self,
        task: Ieee802154TaskOwner,
        interrupts: Ieee802154InterruptSetupOwner,
    ) -> Result<
        (Ieee802154FoundationConfigured, Ieee802154PhySuspended),
        RegisteredIeee802154ReturnFailure<Ieee802154PhySuspended>,
    > {
        match Ieee802154FoundationConfigured::from_operational(Ieee802154Operational {
            task,
            interrupts,
        }) {
            Ok(foundation) => Ok((foundation, self.suspended)),
            Err(failure) => Err(RegisteredIeee802154ReturnFailure {
                failure,
                membership: self.suspended,
            }),
        }
    }
}

/// Failed route sleep or wake retaining the unchanged route.
#[must_use = "a failed route transition still owns the route"]
pub struct RegisteredIeee802154RouteFailure<R> {
    error: Ieee802154PhyClientError,
    route: R,
}

impl<R> RegisteredIeee802154RouteFailure<R> {
    pub const fn error(&self) -> Ieee802154PhyClientError {
        self.error
    }

    /// Recover the unchanged route for a retry.
    pub fn into_route(self) -> R {
        self.route
    }
}

impl<R> fmt::Debug for RegisteredIeee802154RouteFailure<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154RouteFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Failed foundation proof on return, retaining the reset owner and the
/// membership, or the suspended client of a sleeping route.
#[must_use = "a failed return still owns the partition and the PHY membership"]
pub struct RegisteredIeee802154ReturnFailure<M = Ieee802154PhyMembership> {
    failure: Ieee802154FoundationTransitionFailure,
    membership: M,
}

impl<M> RegisteredIeee802154ReturnFailure<M> {
    /// Borrow the foundation readback failure.
    pub const fn failure(&self) -> &Ieee802154FoundationTransitionFailure {
        &self.failure
    }

    /// Recover the reset owner and the membership.
    pub fn into_parts(self) -> (Ieee802154FoundationTransitionFailure, M) {
        (self.failure, self.membership)
    }
}

impl<M> fmt::Debug for RegisteredIeee802154ReturnFailure<M> {
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
impl Ieee802154PhySuspended {
    fn for_test() -> Self {
        Self { _private: () }
    }
}

#[cfg(test)]
mod tests;
