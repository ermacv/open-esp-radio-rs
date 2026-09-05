//! Target-registered common-PHY ownership for the Bluetooth client.
//!
//! Bluetooth already owns its outer Controller and platform epoch. These
//! wrappers therefore retain only the target-issued PHY-registration proof and
//! the source-owned shared-PHY client set. The concrete target runners borrow
//! the outer platform and [`open_esp_radio_esp32s31_hal::SharedPhyHal`] for one
//! terminal operation; neither resource can escape through this module.
//!
//! Registration and Bluetooth-client acquisition are deliberately separate.
//! A completed `register_chipv7_phy` transition does not imply that the
//! Bluetooth client bit was acquired, and a pending initial tracking request
//! must finish before the client owner can advance into BTBB initialization.
//! None of these states claims RF qualification or operational Link Layer
//! readiness.

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{
        PhyClientAcquireError, PhyClientAcquireFailure, PhyClientAcquireOrdering,
        PhyClientAcquireOutcome, PhyClientSnapshot, PhyClientState, PhyModemClient,
        PhyPendingTrack, PhyPendingTracking, PhyPllTrackClock, PhyTrackPoisoned,
    },
    tracking::parameters::{PhyParamTrackRequest, PhyParamTrackingAction},
};

/// Target-registered common PHY before the Bluetooth client is acquired.
///
/// The private fields prevent safe code from replacing the registered state or
/// supplying an unrelated client-set image. This owner is neither `Copy` nor
/// `Clone` and exposes no decomposer.
#[must_use = "the target-registered Bluetooth PHY owner is unique"]
pub struct RegisteredBluetoothPhy {
    registered: RegisteredPhyState,
    clients: PhyClientState,
}

impl RegisteredBluetoothPhy {
    /// Mint the Bluetooth owner after one concrete target registration run.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        state: PhyState,
        witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        Self {
            registered: RegisteredPhyState::from_target_completion(state, witness),
            clients: PhyClientState::for_registered_epoch(
                crate::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
            ),
        }
    }

    /// Borrow the target-registered PHY state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect the source-owned client set without exposing its bit image.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Acquire exactly the Bluetooth shared-PHY client.
    ///
    /// A due initial tracking request remains affine with the registered state
    /// and must pass through the target tracking runner before the caller can
    /// obtain [`RegisteredBluetoothPhyClient`].
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the registered PHY and client owner"
    )]
    pub fn acquire_phy_client(
        self,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<RegisteredBluetoothPhyClientAcquire, RegisteredBluetoothPhyClientAcquireFailure>
    {
        let Self {
            registered,
            clients,
        } = self;
        match clients.acquire(PhyModemClient::Bluetooth, clock) {
            Ok(outcome) => Ok(RegisteredBluetoothPhyClientAcquire {
                registered,
                outcome,
            }),
            Err(failure) => Err(RegisteredBluetoothPhyClientAcquireFailure {
                registered,
                failure,
            }),
        }
    }
}

/// Successful Bluetooth-client acquisition retaining target registration.
#[must_use = "Bluetooth client acquisition retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyClientAcquire {
    registered: RegisteredPhyState,
    outcome: PhyClientAcquireOutcome,
}

impl RegisteredBluetoothPhyClientAcquire {
    /// Return the reviewed first/later-client ordering.
    pub const fn ordering(&self) -> PhyClientAcquireOrdering {
        self.outcome.ordering()
    }

    /// Borrow the immediate tracking request, when one is due.
    pub const fn request(&self) -> Option<&PhyParamTrackRequest> {
        self.outcome.request()
    }

    /// Finish the software acquisition edge or retain pending target work.
    #[allow(
        clippy::result_large_err,
        reason = "pending work retains the allocation-free registered PHY owner"
    )]
    pub fn into_owner(
        self,
    ) -> Result<RegisteredBluetoothPhyClient, RegisteredBluetoothPhyPendingTrack> {
        let Self {
            registered,
            outcome,
        } = self;
        match outcome.into_owner() {
            Ok(clients) => Ok(RegisteredBluetoothPhyClient {
                registered,
                clients,
            }),
            Err(pending) => Err(RegisteredBluetoothPhyPendingTrack {
                registered,
                pending,
            }),
        }
    }
}

/// Rejected Bluetooth-client acquisition retaining the unchanged owner.
#[must_use = "failed Bluetooth client acquisition retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyClientAcquireFailure {
    registered: RegisteredPhyState,
    failure: PhyClientAcquireFailure,
}

impl RegisteredBluetoothPhyClientAcquireFailure {
    /// Inspect the exact source-owned client-set rejection.
    pub const fn error(&self) -> PhyClientAcquireError {
        self.failure.error()
    }

    /// Recover the unchanged pre-acquisition owner.
    pub fn into_owner(self) -> RegisteredBluetoothPhy {
        RegisteredBluetoothPhy {
            registered: self.registered,
            clients: self.failure.into_owner(),
        }
    }
}

/// Target-registered PHY with the Bluetooth client acquired and settled.
///
/// This is the lower owner that a Bluetooth Controller may retain across BTBB
/// and BLE-engine initialization for an always-awake profile. It proves neither
/// a per-event RF-ready instant nor operational radio-engine readiness.
#[must_use = "the registered Bluetooth PHY client owner is unique"]
pub struct RegisteredBluetoothPhyClient {
    registered: RegisteredPhyState,
    clients: PhyClientState,
}

impl RegisteredBluetoothPhyClient {
    /// Borrow the target-registered PHY state without mutable authority.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect the settled source-owned client set.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }
}

/// Pending immediate tracking after Bluetooth-client acquisition.
#[must_use = "pending Bluetooth tracking retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyPendingTrack {
    registered: RegisteredPhyState,
    pending: PhyPendingTrack,
}

impl RegisteredBluetoothPhyPendingTrack {
    /// Borrow the exact source-owned tracking request.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.pending.request()
    }

    /// Begin tracking with policy projected from this registered PHY epoch.
    pub fn begin_tracking(self) -> RegisteredBluetoothPhyPendingTracking {
        let policy = self.registered.tracking_policy();
        RegisteredBluetoothPhyPendingTracking {
            registered: self.registered,
            pending: self.pending.begin_tracking(policy),
        }
    }

    /// Enter fail-stop state without attempting target tracking work.
    pub fn fail(self) -> RegisteredBluetoothPhyTrackPoisoned {
        RegisteredBluetoothPhyTrackPoisoned {
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// In-flight Bluetooth tracking retaining the target registration proof.
#[must_use = "in-flight Bluetooth tracking retains the registered PHY owner"]
pub struct RegisteredBluetoothPhyPendingTracking {
    registered: RegisteredPhyState,
    pending: PhyPendingTracking,
}

impl RegisteredBluetoothPhyPendingTracking {
    /// Inspect the next semantic target operation.
    pub const fn action(&self) -> PhyParamTrackingAction {
        self.pending.action()
    }

    /// Borrow the last committed target-registered PHY state.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_tracking_parts(&mut self) -> (&mut PhyState, &mut PhyPendingTracking) {
        (self.registered.target_state_mut(), &mut self.pending)
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        clippy::result_large_err,
        reason = "incomplete target work retains the allocation-free Bluetooth PHY epoch"
    )]
    pub(crate) fn into_client_owner(self) -> Result<RegisteredBluetoothPhyClient, Self> {
        let Self {
            registered,
            pending,
        } = self;
        match pending.into_owner() {
            Ok(clients) => Ok(RegisteredBluetoothPhyClient {
                registered,
                clients,
            }),
            Err(pending) => Err(Self {
                registered,
                pending,
            }),
        }
    }

    /// Consume ambiguous work into a non-recoverable owner.
    pub fn fail(self) -> RegisteredBluetoothPhyTrackPoisoned {
        RegisteredBluetoothPhyTrackPoisoned {
            registered: self.registered,
            poisoned: self.pending.fail(),
        }
    }
}

/// Fail-stop Bluetooth PHY epoch after ambiguous tracking hardware work.
#[must_use = "failed Bluetooth tracking poisons the registered PHY epoch"]
pub struct RegisteredBluetoothPhyTrackPoisoned {
    registered: RegisteredPhyState,
    poisoned: PhyTrackPoisoned,
}

impl RegisteredBluetoothPhyTrackPoisoned {
    /// Borrow the last committed semantic state for diagnostics.
    pub const fn phy_state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Borrow the exact tracking request which poisoned the epoch.
    pub const fn request(&self) -> &PhyParamTrackRequest {
        self.poisoned.request()
    }

    /// Inspect the retained client set without any recovery authority.
    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.poisoned.snapshot()
    }
}

#[cfg(test)]
mod tests;
