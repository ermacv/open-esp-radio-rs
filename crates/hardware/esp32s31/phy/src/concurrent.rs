//! One registered PHY domain shared by concurrently running protocol clients.
//!
//! [`ConcurrentPhy`] is the PHY layer's attachment to the HAL shared radio
//! arbiter (`SharedRadio<ConcurrentPhy>`). Every operation takes the arbiter's
//! lease, so access to the domain is serialized by the same mechanism that
//! serializes the shared registers, and PHY work always runs on the lease's
//! shared-PHY borrow.
//!
//! The domain is registered once. Wi-Fi, Bluetooth and IEEE 802.15.4 enter and
//! leave it as clients of the one client set. Scheduled tracking and a first
//! acquisition that needs tracking leave the domain *pending*: the tracking
//! transaction may start only when every active client presents a
//! [`ClientQuiescence`] proof, and it must finish before the earliest window
//! any proof grants. A failed or cancelled tracking transaction poisons the
//! domain until reset.
//!
//! The domain also owns its modem clock modules, as ESP-IDF's `esp_phy_enable`
//! and `esp_phy_disable` do: `PHY` is held while RF is open, and
//! `PHY_CALIBRATION` only while registration or RF wake runs. After the last
//! client leaves, closing RF releases `PHY`; waking RF takes it again.

use oer_esp32s31_hal::shared_radio::{
    ClientQuiescence, ModemClockError, QuiescentSpan, RadioClient, SharedRadioLease,
};

use crate::{
    RegisteredPhyState,
    registered_route::PhyDomain,
    state::client::{
        PhyClientAcquireError, PhyClientReleaseError, PhyClientSnapshot, PhyModemClient,
        PhyPendingTrack, PhyPllTrackClock, PhyTrackTimeError,
    },
};

/// State of the shared PHY domain kept under the arbiter.
#[derive(Default)]
pub struct ConcurrentPhy {
    slot: Slot,
}

#[derive(Default)]
pub(crate) enum Slot {
    /// No registration yet.
    #[default]
    Empty,
    /// Registered with RF open, with every active client settled.
    Registered(PhyDomain),
    /// Registered with RF closed and no client.
    RfClosed(PhyDomain),
    /// A tracking request must run before clients continue.
    Pending {
        registered: RegisteredPhyState,
        pending: PhyPendingTrack,
    },
    /// Tracking hardware work failed or was cancelled.
    Poisoned,
}

/// Why a concurrent PHY operation was rejected before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentPhyError {
    /// The domain is not registered.
    NotRegistered,
    /// The domain is already registered.
    AlreadyRegistered,
    /// Tracking is pending; it must run before this operation.
    TrackingPending,
    /// No tracking is pending.
    NoTrackingPending,
    /// Tracking, RF close or RF wake failed earlier; the domain requires
    /// reset.
    Poisoned,
    /// RF is closed; it must be woken before clients enter.
    RfClosed,
    /// RF is open; only a closed domain can be woken.
    RfOpen,
    /// The shared-PHY borrow belongs to another registration; no register
    /// access ran.
    EpochMismatch,
    /// Clients are still active; RF closes only after the last one left.
    ClientsActive,
    /// The domain's modem clock module could not change.
    Clock(ModemClockError),
    /// The client set rejected the acquisition.
    Acquire(PhyClientAcquireError),
    /// The client set rejected the release.
    Release(PhyClientReleaseError),
    /// The tracking clock was rejected.
    Time(PhyTrackTimeError),
    /// An active client presented no quiescence proof.
    MissingQuiescence(PhyModemClient),
    /// The PHY clock is behind a proof's paired sample.
    ClockBehindProof,
    /// A proof's window has already closed.
    WindowClosed,
}

/// Result of a client acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentAcquire {
    /// The client runs; no tracking was due.
    Settled,
    /// The client is recorded, but tracking must run before it transmits.
    TrackingDue,
}

/// Admitted PHY maintenance window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaintenanceAdmission {
    release_by_micros: Option<u64>,
}

impl MaintenanceAdmission {
    /// The instant the shared PHY must be released by, or `None` when every
    /// active client is stopped for as long as its proof lives.
    pub const fn release_by_micros(self) -> Option<u64> {
        self.release_by_micros
    }
}

const fn radio_client(client: PhyModemClient) -> RadioClient {
    match client {
        PhyModemClient::Wifi => RadioClient::Wifi,
        PhyModemClient::Bluetooth => RadioClient::Bluetooth,
        PhyModemClient::Ieee802154 => RadioClient::Ieee802154,
    }
}

const ALL_CLIENTS: [PhyModemClient; 3] = [
    PhyModemClient::Wifi,
    PhyModemClient::Bluetooth,
    PhyModemClient::Ieee802154,
];

/// Admit maintenance when every active client proves quiescence at `now`.
pub(crate) fn admit(
    active: PhyClientSnapshot,
    proofs: &[ClientQuiescence<'_>],
    now_micros: u64,
) -> Result<MaintenanceAdmission, ConcurrentPhyError> {
    let mut release_by: Option<u64> = None;
    for client in ALL_CLIENTS {
        if !active.contains(client) {
            continue;
        }
        let Some(proof) = proofs
            .iter()
            .find(|proof| proof.client() == radio_client(client))
        else {
            return Err(ConcurrentPhyError::MissingQuiescence(client));
        };
        if let QuiescentSpan::Until {
            issued_at_micros,
            release_by_micros,
        } = proof.span()
        {
            if now_micros < issued_at_micros {
                return Err(ConcurrentPhyError::ClockBehindProof);
            }
            if now_micros >= release_by_micros {
                return Err(ConcurrentPhyError::WindowClosed);
            }
            release_by = Some(match release_by {
                Some(earlier) => earlier.min(release_by_micros),
                None => release_by_micros,
            });
        }
    }
    Ok(MaintenanceAdmission {
        release_by_micros: release_by,
    })
}

impl ConcurrentPhy {
    /// An empty slot for a fresh arbiter.
    pub const fn new() -> Self {
        Self { slot: Slot::Empty }
    }

    /// The active client set, when the domain is registered and settled.
    pub fn client_snapshot(&self) -> Option<PhyClientSnapshot> {
        match &self.slot {
            Slot::Registered(domain) => Some(domain.client_snapshot()),
            Slot::RfClosed(domain) => Some(domain.client_snapshot()),
            Slot::Pending { pending, .. } => Some(pending.snapshot()),
            Slot::Empty | Slot::Poisoned => None,
        }
    }

    /// Whether the domain is registered with RF closed.
    pub const fn rf_closed(&self) -> bool {
        matches!(self.slot, Slot::RfClosed(_))
    }

    /// Whether tracking must run before clients continue.
    pub const fn tracking_pending(&self) -> bool {
        matches!(self.slot, Slot::Pending { .. })
    }

    fn settled_domain(&mut self) -> Result<PhyDomain, ConcurrentPhyError> {
        match core::mem::take(&mut self.slot) {
            Slot::Registered(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            closed @ Slot::RfClosed(_) => {
                self.slot = closed;
                Err(ConcurrentPhyError::RfClosed)
            }
            pending @ Slot::Pending { .. } => {
                self.slot = pending;
                Err(ConcurrentPhyError::TrackingPending)
            }
            Slot::Poisoned => {
                self.slot = Slot::Poisoned;
                Err(ConcurrentPhyError::Poisoned)
            }
        }
    }

    /// Borrow the registered domain when every active client is settled.
    pub(crate) fn settled(&self) -> Result<&PhyDomain, ConcurrentPhyError> {
        match &self.slot {
            Slot::Registered(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            Slot::RfClosed(_) => Err(ConcurrentPhyError::RfClosed),
            Slot::Pending { .. } => Err(ConcurrentPhyError::TrackingPending),
            Slot::Poisoned => Err(ConcurrentPhyError::Poisoned),
        }
    }

    pub(crate) fn slot_mut(&mut self) -> &mut Slot {
        &mut self.slot
    }

    /// Take the RF-open domain with no active client, for RF close.
    pub(crate) fn idle_domain(&mut self) -> Result<PhyDomain, ConcurrentPhyError> {
        let domain = self.settled_domain()?;
        if domain.client_snapshot().is_empty() {
            Ok(domain)
        } else {
            self.slot = Slot::Registered(domain);
            Err(ConcurrentPhyError::ClientsActive)
        }
    }

    /// Take the RF-closed domain, for RF wake.
    pub(crate) fn closed_domain(&mut self) -> Result<PhyDomain, ConcurrentPhyError> {
        match core::mem::take(&mut self.slot) {
            Slot::RfClosed(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            Slot::Poisoned => {
                self.slot = Slot::Poisoned;
                Err(ConcurrentPhyError::Poisoned)
            }
            other => {
                self.slot = other;
                Err(ConcurrentPhyError::RfOpen)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn registered_for_test(domain: PhyDomain) -> Self {
        Self {
            slot: Slot::Registered(domain),
        }
    }

    #[cfg(test)]
    pub(crate) fn rf_closed_for_test(domain: PhyDomain) -> Self {
        Self {
            slot: Slot::RfClosed(domain),
        }
    }
}

/// Enter `client` into the shared domain.
///
/// # Errors
///
/// The domain is not registered and settled, or the client set rejects the
/// client (already acquired, clock error). The domain is unchanged.
pub fn acquire_client(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    client: PhyModemClient,
    clock: &mut impl PhyPllTrackClock,
) -> Result<ConcurrentAcquire, ConcurrentPhyError> {
    let phy = lease.attachment_mut();
    let PhyDomain {
        registered,
        clients,
    } = phy.settled_domain()?;
    match clients.acquire(client, clock) {
        Ok(outcome) => match outcome.into_owner() {
            Ok(clients) => {
                phy.slot = Slot::Registered(PhyDomain::new(registered, clients));
                Ok(ConcurrentAcquire::Settled)
            }
            Err(pending) => {
                phy.slot = Slot::Pending {
                    registered,
                    pending,
                };
                Ok(ConcurrentAcquire::TrackingDue)
            }
        },
        Err(failure) => {
            let error = failure.error();
            phy.slot = Slot::Registered(PhyDomain::new(registered, failure.into_owner()));
            Err(ConcurrentPhyError::Acquire(error))
        }
    }
}

/// Remove `client` from the shared domain. Returns whether it was the last.
///
/// # Errors
///
/// The domain is not registered and settled, or the client was not acquired.
/// The domain is unchanged.
pub fn release_client(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    client: PhyModemClient,
) -> Result<bool, ConcurrentPhyError> {
    let phy = lease.attachment_mut();
    let PhyDomain {
        registered,
        clients,
    } = phy.settled_domain()?;
    match clients.release(client) {
        Ok(outcome) => {
            let is_last = outcome.is_last();
            phy.slot = Slot::Registered(PhyDomain::new(registered, outcome.into_owner()));
            Ok(is_last)
        }
        Err(failure) => {
            let error = failure.error();
            phy.slot = Slot::Registered(PhyDomain::new(registered, failure.into_owner()));
            Err(ConcurrentPhyError::Release(error))
        }
    }
}

/// Run one periodic tracking evaluation. Returns whether tracking is now due.
///
/// # Errors
///
/// The domain is not registered and settled, or the clock was rejected; the
/// domain is unchanged.
pub fn evaluate_periodic_tracking(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clock: &mut impl PhyPllTrackClock,
) -> Result<bool, ConcurrentPhyError> {
    let phy = lease.attachment_mut();
    let PhyDomain {
        registered,
        clients,
    } = phy.settled_domain()?;
    match clients.evaluate_periodic_tracking(clock) {
        Ok(evaluation) => match evaluation.into_owner() {
            Ok(clients) => {
                phy.slot = Slot::Registered(PhyDomain::new(registered, clients));
                Ok(false)
            }
            Err(pending) => {
                phy.slot = Slot::Pending {
                    registered,
                    pending,
                };
                Ok(true)
            }
        },
        Err(failure) => {
            let error = failure.error();
            phy.slot = Slot::Registered(PhyDomain::new(registered, failure.into_owner()));
            Err(ConcurrentPhyError::Time(error))
        }
    }
}

/// Check, without register access, whether pending tracking may start now.
///
/// # Errors
///
/// No tracking is pending, an active client presented no proof, or a proof's
/// window is not open at `now_micros`.
pub fn admit_maintenance(
    lease: &SharedRadioLease<'_, ConcurrentPhy>,
    proofs: &[ClientQuiescence<'_>],
    now_micros: u64,
) -> Result<MaintenanceAdmission, ConcurrentPhyError> {
    match &lease.attachment().slot {
        Slot::Pending { pending, .. } => admit(pending.snapshot(), proofs, now_micros),
        Slot::Poisoned => Err(ConcurrentPhyError::Poisoned),
        Slot::RfClosed(_) => Err(ConcurrentPhyError::RfClosed),
        Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
        Slot::Registered(_) => Err(ConcurrentPhyError::NoTrackingPending),
    }
}

#[cfg(test)]
mod tests;
