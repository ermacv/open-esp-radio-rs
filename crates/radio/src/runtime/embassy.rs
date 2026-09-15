//! Embassy mailbox for the hardware-free radio controller.
//!
//! The public path remains stable while private domains keep bounded message
//! values, transport cancellation, stopped planning, active-role polling and
//! the sole owner-holding actor separate. Only the actor/runner domain owns
//! physical role frontiers; the mailbox transports value requests and reports.

mod active;
mod actor;
mod dispatch;
mod message;
mod transport;

pub use active::{
    EmbassyWifiActiveRoleControl, EmbassyWifiActiveRoleExit, EmbassyWifiRoleFrontier,
    drive_embassy_wifi_active_role, drive_embassy_wifi_active_role_pinned,
    finish_embassy_wifi_active_role,
};
pub use actor::{
    EmbassyWifiRoleEpochOutcome, EmbassyWifiRoleEpochRunner, EmbassyWifiSupervisorPrepareFailure,
    EmbassyWifiSupervisorPrepared, EmbassyWifiSupervisorTask, prepare_embassy_wifi_supervisor,
    run_embassy_wifi_supervisor_actor,
};
pub use dispatch::{EmbassyWifiStoppedDispatch, dispatch_embassy_wifi_stopped_command};
pub use message::{
    EmbassyWifiStartKind, EmbassyWifiSupervisorCommand, EmbassyWifiSupervisorError,
    EmbassyWifiSupervisorResponse,
};
pub use transport::{
    EmbassyWifiSupervisorControlError, EmbassyWifiSupervisorControlResources,
    EmbassyWifiSupervisorEndpoint, EmbassyWifiSupervisorEndpoints, EmbassyWifiSupervisorPort,
};

#[cfg(test)]
mod tests;
