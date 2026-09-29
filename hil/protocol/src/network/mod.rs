//! The network module: measured transport sessions over a Wi-Fi role, their
//! lifecycle and evidence, with the properties network images advertise.

#[cfg(feature = "wifi")]
use postcard_schema::Schema;
#[cfg(feature = "wifi")]
use serde::{Deserialize, Serialize};

mod state;
pub use state::*;
#[cfg(feature = "wifi")]
mod session;
#[cfg(feature = "wifi")]
pub use session::*;
#[cfg(feature = "wifi")]
mod traffic;
#[cfg(feature = "wifi")]
pub use traffic::*;
#[cfg(feature = "wifi")]
mod stream_pattern;
#[cfg(feature = "wifi")]
pub use stream_pattern::*;
#[cfg(feature = "wifi")]
mod udp_probe;
#[cfg(feature = "wifi")]
pub use udp_probe::*;

crate::messages! {
    property Udp = "network/udp";
    property Tcp = "network/tcp";
    property Rx = "network/rx";
    property Tx = "network/tx";
    property Bidirectional = "network/bidirectional";
    /// Session configuration at run time.
    property RuntimeConfiguration = "network/runtime-configuration";
    /// Sessions end with typed evidence records.
    property StructuredEvidence = "network/structured-evidence";
    /// One UDP session executes and accounts more than one peer flow.
    property UdpMultiFlow = "network/udp-multi-flow";
    /// UDP RX sessions return typed evidence for every delivery frontier
    /// from post-reorder publication through the application socket.
    property RxDeliveryReport = "network/rx-delivery-evidence";
    /// Aggregate cooperative network scheduler evidence.
    property SchedulerEvidence = "network/scheduler-evidence";
    /// Startup provisioning selects the data-plane executor topology.
    property DataPlanePlacement = "network/data-plane-placement";
    #[cfg(feature = "wifi")]
    endpoint Configure = "network/session/configure" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint Arm = "network/session/arm" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint Start = "network/session/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint Cancel = "network/session/cancel" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint Recover = "network/session/recover" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint ReplayResult = "network/session/result/replay" => crate::network::Finished;
    #[cfg(feature = "wifi")]
    endpoint AcknowledgeResult = "network/session/result/acknowledge" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    endpoint GetStatus = "network/session/status/get" => crate::network::Status;
    #[cfg(feature = "wifi")]
    topic Status = "network/session/status";
    #[cfg(feature = "wifi")]
    topic StateChanged = "network/session/state";
    #[cfg(feature = "wifi")]
    topic Ready = "network/ready";
    #[cfg(feature = "wifi")]
    topic ServiceReady = "network/service/ready";
    #[cfg(feature = "wifi")]
    topic SessionReady = "network/session/ready";
    #[cfg(feature = "wifi")]
    topic UdpRxStarted = "network/session/udp-rx-started";
    #[cfg(feature = "wifi")]
    topic Evidence = "network/session/evidence";
    #[cfg(feature = "wifi")]
    topic Finished = "network/session/finished";
    #[cfg(feature = "wifi")]
    topic Failed = "network/session/failed";
}

/// `network/session/configure`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Configure(pub crate::network::SessionConfig);

/// `network/session/arm`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Arm;

/// `network/session/start`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Start;

/// Cancel a configured or armed session before any workload starts.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Cancel;

/// Explicitly discard a terminal result and return to idle ownership.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Recover;

/// Replay the retained evidence for a completed session.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReplayResult;

/// `network/session/result/acknowledge`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AcknowledgeResult;

/// Return the current operation state and retained session identities.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetStatus;

/// Correlated response to its request.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Status(pub crate::network::OperationStatus);

/// `network/session/state`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StateChanged(pub crate::network::StateChange);

/// `network/ready`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Ready(pub crate::network::NetworkInfo);

/// `network/service/ready`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ServiceReady(pub crate::network::ServiceInfo);

/// Single-flow UDP consumer reached its first 256 valid data packets.
/// Session-correlated delivery, distinct from socket readiness or host send.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UdpRxStarted {
    pub datagrams: u64,
}

/// `network/session/evidence`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Evidence(pub crate::network::EvidenceRecord);

/// `network/session/failed`.
#[cfg(feature = "wifi")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Failed(pub crate::network::FailureCode);

#[cfg(all(test, feature = "wifi"))]
mod tests;
