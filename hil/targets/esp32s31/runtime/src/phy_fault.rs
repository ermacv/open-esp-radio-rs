//! Wire adaptation only. Production PHY owns checkpoints, composition owns
//! the deadline, and no diagnostic command can renew that deadline.
use oer_hil_protocol::{Event, PhyFaultCommand, RejectReason};

pub(super) fn control(command: PhyFaultCommand) -> Event {
    #[cfg(feature = "phy-fault-injection")]
    {
        use oer_esp32s31_phy::fault_injection::{self as fault, Mode, Phase};
        use oer_hil_protocol::{
            PhyFaultEvidence, PhyFaultMode as WireMode, PhyFaultPhase as WirePhase,
        };
        let accepted = match command {
            PhyFaultCommand::Arm(mode) => fault::arm(match mode {
                WireMode::BlockedPoll => Mode::BlockedPoll,
                WireMode::LostCompletion => Mode::LostCompletion,
                WireMode::Restoration => Mode::Restoration,
                WireMode::Cancelled => Mode::Cancelled,
            }),
            PhyFaultCommand::Status => true,
            // The transport commits release only after this acknowledgement
            // has actually been serialized, before the CPU can block.
            PhyFaultCommand::Release => fault::phase() == Phase::Reached,
        };
        if !accepted {
            return Event::Rejected(RejectReason::InvalidState);
        }
        Event::PhyFault(PhyFaultEvidence {
            phase: match if command == PhyFaultCommand::Release {
                Phase::Released
            } else {
                fault::phase()
            } {
                Phase::Idle => WirePhase::Idle,
                Phase::Armed => WirePhase::Armed,
                Phase::Reached => WirePhase::Reached,
                Phase::Released => WirePhase::Released,
                Phase::Cancelled => WirePhase::Cancelled,
            },
            reset_reason: crate::system::boot_evidence().reset_reason,
        })
    }
    #[cfg(not(feature = "phy-fault-injection"))]
    {
        let _ = command;
        Event::Rejected(RejectReason::InvalidState)
    }
}

pub(super) fn after_response(command: PhyFaultCommand, accepted: bool) {
    #[cfg(feature = "phy-fault-injection")]
    if accepted && command == PhyFaultCommand::Release {
        assert!(oer_esp32s31_phy::fault_injection::release());
    }
    #[cfg(not(feature = "phy-fault-injection"))]
    let _ = (command, accepted);
}
