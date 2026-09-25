//! Registered PHY state retained by the Wi-Fi runtime owner.
//!
//! The MAC/DMA capabilities move into role tasks separately. This value stays
//! with their owning runtime context and is returned at the same stopped
//! frontier. Channel operations borrow that frontier's HAL; the semantic state
//! and client scheduler cannot be replaced by callers.

use oer_esp32s31_hal::owner::{Radio, state::Powered};

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{PhyClientReleaseError, PhyClientSnapshot, PhyClientState, PhyModemClient},
};

/// Wi-Fi's retained registration and client scheduler, without a raw-state
/// constructor or mutable state escape.
///
/// ```compile_fail
/// use oer_esp32s31_phy::{RegisteredWifiPhy, PhyState};
/// fn replace(phy: &mut RegisteredWifiPhy, replacement: PhyState) {
///     *phy.state_mut() = replacement;
/// }
/// ```
#[must_use = "registered Wi-Fi PHY belongs to its runtime hardware epoch"]
pub struct RegisteredWifiPhy {
    pub(crate) registered: RegisteredPhyState,
    pub(crate) clients: PhyClientState,
}

/// A due periodic pass or an explicitly selected operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPhyMaintenanceRequest {
    /// Apply the registered temperature policy.
    Track,
    /// Execute one selected operation after physical admission. Thermal
    /// predicates remain active; the periodic evaluation deadline is unchanged.
    Operation(crate::tracking::maintenance::Operation),
    /// Selected work rechecks sample age after physical admission.
    /// A stale sample returns without executing or acknowledging the request.
    ObservedOperation {
        operation: crate::tracking::maintenance::Operation,
        maximum_age_micros: u64,
    },
    /// Run common and Wi-Fi calibration at the next due pass, even without
    /// a temperature change. The zero threshold applies only to this pass;
    /// sensor readings, timestamps and the registered policy are unchanged.
    Calibrate,
    /// Diagnostic zero-threshold measurement of the common branch alone.
    CalibrateCommon,
    /// Diagnostic zero-threshold measurement of the Wi-Fi TX branch alone.
    CalibrateTransmit,
    /// One measured RFPLL correction with zero thermal threshold under exclusive
    /// maintenance access. Does not enable periodic RFPLL or advance its deadline.
    /// Requires a recent completed sensor acquisition after physical admission.
    MeasureRfpll { maximum_age_micros: u64 },
}

impl RegisteredWifiPhy {
    pub const fn state(&self) -> &PhyState {
        self.registered.state()
    }

    /// Inspect registered-policy conditions without sampling temperature, advancing
    /// deadlines or acquiring RF. Values describe retained state, not a job plan.
    pub fn inspect_tracking(
        &self,
        now_micros: u64,
    ) -> Result<crate::tracking::inspection::Inspection, crate::state::client::PhyTrackTimeError>
    {
        crate::tracking::inspection::Inspection::registered(
            &self.registered,
            self.client_snapshot(),
            now_micros,
        )
    }

    pub const fn client_snapshot(&self) -> PhyClientSnapshot {
        self.clients.snapshot()
    }

    /// Release the Wi-Fi client and reunite the registration with the powered
    /// radio returned by the same stopped runtime frontier.
    ///
    /// The registration must still describe that radio's PHY partition, and
    /// the Wi-Fi client must be present. Either rejection returns this owner
    /// and the radio unchanged.
    #[allow(
        clippy::result_large_err,
        reason = "failure must retain the allocation-free registered PHY owner and radio"
    )]
    pub fn release_wifi_client<P>(
        self,
        mut radio: Radio<P, Powered>,
    ) -> Result<crate::RegisteredPhyClientRelease<P>, RegisteredWifiPhyClientReleaseFailure<P>>
    {
        if !self.clients.describes(&*radio.phy_hal_mut()) {
            return Err(RegisteredWifiPhyClientReleaseFailure {
                owner: self,
                radio,
                error: RegisteredWifiPhyClientReleaseError::EpochMismatch,
            });
        }
        let Self {
            registered,
            clients,
        } = self;
        match clients.release(PhyModemClient::Wifi) {
            Ok(outcome) => Ok(crate::RegisteredPhyClientRelease::from_detached_parts(
                radio, registered, outcome,
            )),
            Err(failure) => Err(RegisteredWifiPhyClientReleaseFailure {
                error: RegisteredWifiPhyClientReleaseError::Client(failure.error()),
                owner: Self {
                    registered,
                    clients: failure.into_owner(),
                },
                radio,
            }),
        }
    }
}

/// Why the Wi-Fi client could not be released into its powered radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisteredWifiPhyClientReleaseError {
    /// The registration no longer describes the radio's PHY partition.
    EpochMismatch,
    /// The client set rejected the release.
    Client(PhyClientReleaseError),
}

/// Rejected Wi-Fi client release retaining the unchanged owner and radio.
#[must_use = "failed release retains the registered PHY owner and the powered radio"]
pub struct RegisteredWifiPhyClientReleaseFailure<P> {
    owner: RegisteredWifiPhy,
    radio: Radio<P, Powered>,
    error: RegisteredWifiPhyClientReleaseError,
}

impl<P> RegisteredWifiPhyClientReleaseFailure<P> {
    pub const fn error(&self) -> RegisteredWifiPhyClientReleaseError {
        self.error
    }

    /// Return the unchanged registered owner and powered radio.
    pub fn into_parts(self) -> (RegisteredWifiPhy, Radio<P, Powered>) {
        (self.owner, self.radio)
    }
}

// Scheduler evaluation is also compiled on the host: the same owner transition
// is exercised without manufacturing a hardware completion.
#[cfg(any(target_arch = "riscv32", test))]
mod maintenance;
#[cfg(target_arch = "riscv32")]
pub use maintenance::{WifiPhyMaintenanceError, WifiPhyMaintenanceFailure};
