//! Shared PHY diagnostic control, independent of the selected radio protocol.
use serde::{Deserialize, Serialize};
/// Destructive injection into real PHY maintenance, not the idle SoC test.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PhyFaultMode {
    BlockedPoll,
    LostCompletion,
    Restoration,
    Cancelled,
}

/// A one-shot attempt cannot be disarmed or replaced before reboot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PhyFaultCommand {
    Arm(PhyFaultMode),
    Status,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PhyFaultPhase {
    Idle,
    Armed,
    Reached,
    Released,
    Cancelled,
}

/// Reset cause is reported independently of the fault's checkpoint phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyFaultEvidence {
    pub phase: PhyFaultPhase,
    pub reset_reason: crate::ResetReason,
}
