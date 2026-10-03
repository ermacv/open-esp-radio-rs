//! The telemetry module: the reset-retained event trace, the program-counter
//! profile, and the properties of the diagnostic observers an image compiles
//! in.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod profile;
pub use profile::*;
mod trace;
pub use trace::*;

crate::messages! {
    /// Physical RX allocation lifetime observations.
    property RxOwnership = "telemetry/rx-ownership";
    /// The Wi-Fi system's diagnostics, including the station's exit
    /// evidence, without the other driver observers.
    property StationExit = "telemetry/station-exit";
    /// The program-counter profile: a sampling interrupt on each hart.
    property PcProfile = "telemetry/pc-profile";
    /// The receive-clock probe: frame receive timestamps beside their
    /// handoff time and paired MAC local-time readings, logged to the
    /// console.
    property RxClock = "telemetry/rx-clock";
    /// Bounded Embassy task poll residence.
    property TaskPoll = "telemetry/task-poll";
    /// The intrusive Core0 RX phase and service histograms.
    property Core0RxCycles = "telemetry/core0-rx-cycles";
    /// MAC interrupt publication timestamps sampled in the hard ISR.
    property MacIrq = "telemetry/mac-irq";
    endpoint ControlTrace = "telemetry/trace/control" => crate::telemetry::TraceState;
    topic TraceState = "telemetry/trace/state";
    endpoint GetTraceEntries = "telemetry/trace/entries/get" => crate::telemetry::TraceEntriesPage;
    topic TraceEntriesPage = "telemetry/trace/entries";
    endpoint GetTraceSnapshot = "telemetry/trace/snapshot/get" => crate::telemetry::TraceSnapshot;
    topic TraceSnapshot = "telemetry/trace/snapshot";
    endpoint ControlProfile = "telemetry/profile/control" => crate::telemetry::ProfileState;
    topic ProfileState = "telemetry/profile/state";
    endpoint GetProfileSamples = "telemetry/profile/samples/get" => crate::telemetry::ProfileSamples;
    topic ProfileSamples = "telemetry/profile/samples";
}

/// Report, start or re-mask the reset-retained event trace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ControlTrace(pub crate::telemetry::TraceControl);

/// `telemetry/trace/state`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TraceState(pub crate::telemetry::TraceStatus);

/// Page through the trace's storage slots from `first`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetTraceEntries {
    pub first: u16,
}

/// `telemetry/trace/entries`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TraceEntriesPage(pub crate::telemetry::TraceEntries);

/// Page through the snapshot in `slot` from word `offset`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetTraceSnapshot {
    pub slot: u8,
    pub offset: u16,
}

/// `None` when the slot holds no snapshot, or another one by now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TraceSnapshot(pub Option<crate::telemetry::TraceSnapshotPage>);

/// Arm, disarm or query the program-counter profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ControlProfile(pub crate::telemetry::ProfileControl);

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProfileState(pub crate::telemetry::ProfileStatus);

/// Read retained profile samples of one hart from `first`; only a closed
/// window has pages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetProfileSamples {
    pub hart: u8,
    pub first: u32,
}

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProfileSamples(pub crate::telemetry::ProfileSamplesPage);
