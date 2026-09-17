//! Terminal policy for the shared PHY epoch, independent of the stop mechanism.
//!
//! This decision is not itself proof that RF stopped. Its platform consumer must
//! establish TX/RX and DMA/IRQ quiescence and prevent every protocol from
//! reacquiring the epoch. An active DTM test is abandoned, never paused or
//! completed by synthesizing a Host Test End. If local quiescence cannot be
//! proven, the platform must escalate to a full system reset. Host progress
//! must never be a prerequisite for that stop.

/// The shared PHY epoch may no longer authorize RF work by any protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "establish proven shared RF quiescence or reset the platform"]
pub enum SharedPhyFailStop {
    MaintenanceHardDeadlineExceeded,
    /// Maintenance or restoration became ambiguous; ordinary recovery is forbidden.
    MaintenanceFailed,
}
