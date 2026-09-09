//! Registered PHY state retained by the Wi-Fi runtime owner.
//!
//! The MAC/DMA capabilities move into role tasks separately. This value stays
//! with their owning runtime context and is returned at the same stopped
//! frontier. Channel operations borrow that frontier's HAL; the semantic state
//! and client scheduler cannot be replaced by callers.

use crate::{
    PhyState, RegisteredPhyState,
    state::client::{PhyClientSnapshot, PhyClientState},
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

/// One maintenance pass, still subject to the real tracking deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPhyMaintenanceRequest {
    /// Apply the registered temperature policy.
    Track,
    /// Run common and Wi-Fi calibration at the next due pass, even without
    /// a temperature change. The zero threshold applies only to this pass;
    /// sensor readings, timestamps and the registered policy are unchanged.
    Calibrate,
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
}

// Scheduler evaluation is also compiled on the host: the same owner transition
// is exercised without manufacturing a hardware completion.
#[cfg(any(target_arch = "riscv32", test))]
mod maintenance;
#[cfg(target_arch = "riscv32")]
pub use maintenance::{WifiPhyMaintenanceError, WifiPhyMaintenanceFailure};
