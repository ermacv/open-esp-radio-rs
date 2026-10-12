#![no_std]
#![forbid(unsafe_code)]

//! Sans-IO Bluetooth LE Controller core.
//!
//! [`LeController`] serves the Host's HCI commands and runs the Link Layer
//! roles over the portable radio event contract of `oer-bluetooth-radio`. It
//! owns no transport, executor, clock or hardware: its owner moves command
//! packets in, response and event packets out, and radio requests and
//! outcomes between the roles and one radio backend.
//!
//! The roles run concurrently where the specification allows:
//!
//! - legacy advertising: non-connectable, scannable or connectable with its
//!   scan response, with the pseudo-random advertising delay and live
//!   advertising-data updates, and connectable directed at high or low duty
//!   cycle;
//! - peripheral connections, up to the `CONNECTIONS` the composition sets,
//!   each created by a connection indication to the connectable set, which
//!   runs only while a connection slot is free: connection events widened
//!   for clock drift, supervision
//!   and establishment timeouts, Channel Map and Connection Update instants,
//!   feature, version, ping and termination procedures, LE encryption start,
//!   pause and restart, and ACL data both ways with Host flow control;
//! - legacy passive and active scanning with LE Advertising Reports and the
//!   duplicate filter, reporting every advertiser or only the filter accept
//!   list, whose Clear, Add and Remove commands the radio backend executes;
//! - Direct Test Mode transmitter and receiver tests, which run alone.
//!
//! One air scheduler places every event so that no two reservations
//! overlap, by class: control work first, then connection events, then
//! advertising, then scanning. A connection event never moves: the
//! connection closest to its supervision timeout, the one served longest ago
//! among equals, takes its event first; a less urgent connection's event in
//! its way is cancelled, as is an advertising event or scan window submitted
//! where a connection's next event now lies, and an event that still finds
//! another connection's event in progress is missed. A scan window starts
//! after busy reservations and ends before the next advertising or
//! connection event, and an advertising event that cannot start within the
//! advertising delay range is skipped. Connections use no peripheral latency
//! and no PHY other than LE 1M, and connectable advertising needs the random
//! source that encryption draws from.
//!
//! A connection's slot is internal; the backend names it by the slot's
//! `ConnectionId`, and the Host by a connection handle the core allocates
//! and does not reuse at once. Host ACL data is admitted per handle
//! ([`AclAdmission`]), and Controller-to-Host data shares the Host's buffers:
//! each connection holds what the Host has not completed.
//!
//! Schedule calculations use checked radio-domain arithmetic. `next_request`
//! returns `Ok(None)` for ordinary absence of work or a bounded placement
//! conflict, and [`EpochExhausted`], with the role, operation and
//! calculation, when a required continuation lies outside the radio epoch:
//! its only failure. Calculation precedes committing provisional owners;
//! cancellation and release requests do not require a future anchor. The
//! service then ends through its managed stop, which settles the retained
//! core and admitted identities before memory is returned.
//!
//! Only a captured packet start opens or updates a connection time
//! reference: a PDU without one, and an event that ended `Aborted`, prove no
//! anchor.
//!
//! Commands complete in order. Reset, advertising and scanning enable
//! changes, filter accept list changes, advertising-data updates while
//! advertising and Test End complete
//! only after their radio work has finished; until then
//! [`LeController::is_command_ready`] is false. Host ACL packets enter through
//! [`LeController::acl`] for the handles [`LeController::acl_admission`] admits.

#[cfg(test)]
extern crate std;

mod accept_list;
mod advertising;
mod arbiter;
mod coexistence;
mod controller;
#[cfg(feature = "diagnostic-mic-fault")]
mod diagnostic;
mod dtm;
mod output;
mod peripheral;
mod planning;
mod scanning;
mod trace;

pub use controller::{
    AclAdmission, ControllerBusy, LeController, LeControllerConfig, PLANNING_SLACK, RadioWork,
    serves_the_core,
};
pub use oer_bluetooth_ll::control::LeVersionInformation;
pub use output::{HCI_PACKET_CAPACITY, HciPacket};
pub use planning::{EpochExhausted, PlanningCalculation, PlanningOperation, PlanningRole};
pub use scanning::{DUPLICATE_FILTER_CAPACITY, MINIMUM_SCAN_WINDOW};

#[cfg(test)]
mod tests;
