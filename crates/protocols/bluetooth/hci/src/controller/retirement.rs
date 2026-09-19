//! Lossless retirement of command authority after both transport FIFOs drain.

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::{HciEpochIdentity, LeControllerCommandEndpoint, LeControllerCommandReady};

/// Why a graceful HCI retirement cannot complete at this boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerHciRetirementError {
    /// The supplied command authority belongs to a different live channel.
    EndpointMismatch,
    /// Accepted Host commands, ACL data or credit returns still need consumption.
    HostPacketsPending,
    /// Published responses or data still await Host consumption.
    ControllerPacketsPending,
    /// Terminal closure already occurred; it cannot become graceful retirement.
    Closed,
}

/// Rejection before reusing a gracefully retired HCI channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerHciRestartError {
    /// The proof or queue generation belongs to another epoch.
    EpochMismatch,
    /// Both original queues must remain empty and closed.
    NotRetired,
    /// Epoch identities cannot wrap and make an old Host handle valid again.
    GenerationExhausted,
}

/// Retired command authority and its unchanged independent owner.
///
/// Creation proves that both packet FIFOs were empty when admission closed and
/// that the epoch's sole next-command authority was consumed. It proves no
/// radio, scheduler, timer, Host-stack or physical-owner quiescence. The outer
/// lifecycle must establish those obligations before releasing hardware.
#[must_use = "retain the owner and retired HCI epoch for lifecycle reunification"]
pub struct LeControllerHciRetired<'epoch, Owner> {
    owner: Owner,
    epoch: HciEpochIdentity<'epoch>,
}

impl<'epoch, Owner> LeControllerHciRetired<'epoch, Owner> {
    /// Borrow the owner returned by the successful transport barrier.
    pub const fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Separate the owner while retaining proof of its retired HCI epoch.
    pub fn into_parts(self) -> (Owner, LeControllerHciRetired<'epoch, ()>) {
        (
            self.owner,
            LeControllerHciRetired {
                owner: (),
                epoch: self.epoch,
            },
        )
    }

    /// Whether this proof belongs to the observed stable HCI identity.
    /// The identity itself grants no command or shutdown authority.
    pub fn matches_epoch(&self, epoch: HciEpochIdentity<'_>) -> bool {
        self.epoch.same_epoch(epoch)
    }

    /// Whether this proof belongs to the supplied closed Controller endpoint.
    pub fn matches_endpoint<M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>(
        &self,
        endpoint: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PC>,
    ) -> bool {
        self.epoch.same_epoch(endpoint.transport().epoch_identity())
    }
}

impl<'resources, M: RawMutex, const H2C: usize, const C2H: usize, const PC: usize>
    LeControllerCommandEndpoint<'resources, M, H2C, C2H, PC>
{
    /// Check exact closed-epoch admission without changing queues or authority.
    /// Holding the sole endpoint and retirement proof across a hardware restart
    /// prevents generation changes; old closed Host handles cannot add packets.
    pub fn check_restart_transport(
        &self,
        retired: &LeControllerHciRetired<'resources, ()>,
    ) -> Result<(), LeControllerHciRestartError> {
        if !retired.matches_endpoint(self) {
            return Err(LeControllerHciRestartError::EpochMismatch);
        }
        self.transport.check_restart()
    }

    /// Start a fresh protocol epoch using the same queue and configuration storage.
    ///
    /// The exact graceful-retirement proof is consumed. Existing Host handles,
    /// ACL credit senders and pending transport futures remain closed forever;
    /// only the returned Host can use the new generation. The bootstrap profile
    /// and installed random-source borrow are preserved while command state,
    /// masks and role configuration restart. Rebinding that source is rejected.
    /// This proves no hardware restart: the outer owner must establish hardware
    /// readiness before it issues new command authority or runs the Controller.
    pub fn restart_transport(
        &mut self,
        retired: LeControllerHciRetired<'resources, ()>,
    ) -> Result<
        crate::InProcessHciHostTransport<'resources, M, H2C, C2H, PC>,
        (
            LeControllerHciRestartError,
            LeControllerHciRetired<'resources, ()>,
        ),
    > {
        if !retired.matches_endpoint(self) {
            return Err((LeControllerHciRestartError::EpochMismatch, retired));
        }
        let host = match self.transport.restart() {
            Ok(host) => host,
            Err(error) => return Err((error, retired)),
        };
        *self.bootstrap = crate::LeControllerBootstrap::new(self.bootstrap.config());
        self.legacy_advertising.reset();
        self.legacy_scanning.reset();
        *self.initial_ready_available = true;
        Ok(host)
    }

    /// Observe the stable epoch identity without borrowing this endpoint itself.
    pub fn epoch_identity(&self) -> HciEpochIdentity<'resources> {
        self.transport().epoch_identity()
    }

    /// Wait until both FIFOs are empty, or report terminal closure.
    ///
    /// This observation borrows channel storage, leaving the endpoint available
    /// for normal command service. It grants no command or shutdown authority:
    /// a concurrent publication may invalidate readiness before retirement.
    /// Dropping this wait consumes nothing; a later wait rechecks both queues.
    /// Use one retirement waiter per epoch. Packet and capacity waiters use
    /// separate registrations and continue to make progress alongside it.
    pub fn wait_retirement_ready(
        &self,
    ) -> impl core::future::Future<Output = Result<(), LeControllerHciRetirementError>>
    + use<'resources, M, H2C, C2H, PC> {
        self.transport.wait_retirement_ready()
    }

    /// Retire next-command authority only when both packet FIFOs are empty.
    ///
    /// Affinity is checked before any mutation. Queue inspection and closure
    /// share both queue locks, so a concurrent Host write either enters the FIFO
    /// and prevents retirement, or observes `Closed`. Rejection returns the
    /// exact command-ready owner; it neither discards packets nor restricts
    /// admission, including the separate Host ACL credit sender.
    ///
    /// The caller must continue driving accepted work and Host event reads
    /// before retrying. This finite method does not request Reset or stop a
    /// radio role. A ready token can coexist with an active radio session; its
    /// owner must discharge that independent obligation before hardware release.
    #[allow(
        clippy::result_large_err,
        reason = "rejection retains the exact affine owner"
    )]
    pub fn try_retire_transport<'epoch, Owner>(
        &mut self,
        ready: LeControllerCommandReady<'epoch, Owner>,
    ) -> Result<
        LeControllerHciRetired<'epoch, Owner>,
        (
            LeControllerHciRetirementError,
            LeControllerCommandReady<'epoch, Owner>,
        ),
    > {
        if !ready.accepts_endpoint(self) {
            return Err((LeControllerHciRetirementError::EndpointMismatch, ready));
        }
        if let Err(error) = self.transport.try_retire() {
            return Err((error, ready));
        }
        let (owner, authority) = ready.into_parts();
        Ok(LeControllerHciRetired {
            owner,
            epoch: authority.epoch_identity(),
        })
    }
}

#[cfg(test)]
mod tests;
