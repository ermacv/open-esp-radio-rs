//! Stand-ins for the payloads of radio families an image leaves out.
//!
//! The command, event and evidence enums keep every family's variants in one
//! fixed order, because the wire encodes a variant by its position. A family
//! whose feature is off does not compile its modules; its payload types are
//! then [`Absent`], so its variants keep their positions but can be neither
//! built nor decoded, and an image receiving one answers with a typed
//! rejection.

use serde::{Deserialize, Serialize};

/// A payload of a radio family this build leaves out: it has no values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Absent {}

/// The payload type names of every family that is off.
pub mod stand_ins {
    #[cfg(not(feature = "wifi"))]
    pub use super::wifi::*;

    #[cfg(not(feature = "bluetooth"))]
    pub use super::bluetooth::*;

    #[cfg(not(feature = "ieee802154"))]
    pub use super::ieee802154::*;

    #[cfg(not(feature = "system"))]
    pub use super::system::*;
}

#[cfg(not(feature = "wifi"))]
mod wifi {
    use super::Absent;

    pub type Finished = Absent;
    pub type FlowTransportEvidence = Absent;
    pub type InitializationConfiguration = Absent;
    pub type NetworkCredentials = Absent;
    pub type NetworkInfo = Absent;
    pub type NetworkSchedulerEvidence = Absent;
    pub type RadioEvidence = Absent;
    pub type RxDeliveryEvidence = Absent;
    pub type RxZeroCopyEvidence = Absent;
    pub type ServiceInfo = Absent;
    pub type SessionConfig = Absent;
    pub type SessionReady = Absent;
    pub type StationEpochEvidence = Absent;
    pub type StationLifecycleEvent = Absent;
    pub type TransportEvidence = Absent;
    pub type TxAggregateTimingEvidence = Absent;
    pub type WifiAccessPointEvidence = Absent;
    pub type WifiAccessPointRequest = Absent;
    pub type WifiAirtimePeerEvidence = Absent;
    pub type WifiAirtimeReport = Absent;
    pub type WifiMonitorCaptureRequest = Absent;
    pub type WifiMonitorEvidence = Absent;
    pub type WifiMonitorFrameChunk = Absent;
    pub type WifiMonitorRequest = Absent;
    pub type WifiRadioRestartEvidence = Absent;
    pub type WifiRoleFailureEvidence = Absent;
    pub type WifiRoleTransitionEvidence = Absent;
    pub type WifiScanEvidence = Absent;
    pub type WifiScanRequest = Absent;
    pub type WifiStationAccessPointRequest = Absent;
    pub type WifiStationAccessPointStopEvidence = Absent;
}

#[cfg(not(feature = "bluetooth"))]
mod bluetooth {
    use super::Absent;

    pub type BluetoothDtmEvidence = Absent;
    pub type BluetoothDtmOperation = Absent;
    pub type BluetoothGattEvidence = Absent;
    pub type BluetoothHciRequest = Absent;
    pub type BluetoothHciResponse = Absent;
    pub type BluetoothNumericDecision = Absent;
    pub type BluetoothSecureGattEvidence = Absent;
}

#[cfg(not(feature = "ieee802154"))]
mod ieee802154 {
    use super::Absent;

    pub type Ieee802154AirCheckEvidence = Absent;
    pub type Ieee802154AirCheckRequest = Absent;
    pub type Ieee802154EdEventProbeEvidence = Absent;
    pub type Ieee802154EdEventProbeRequest = Absent;
    pub type Ieee802154EventStatusProbeEvidence = Absent;
    pub type Ieee802154EventStatusProbeRequest = Absent;
    pub type Ieee802154RouteProbeEvidence = Absent;
    pub type Ieee802154RouteProbeRequest = Absent;
    pub type Ieee802154SessionAssessRequest = Absent;
    pub type Ieee802154SessionAssessment = Absent;
    pub type Ieee802154SessionConfig = Absent;
    pub type Ieee802154SessionPendingRequest = Absent;
    pub type Ieee802154SessionPhyMaintenance = Absent;
    pub type Ieee802154SessionReceiveEvidence = Absent;
    pub type Ieee802154SessionRecentRssi = Absent;
    pub type Ieee802154SessionRestartEvidence = Absent;
    pub type Ieee802154SessionResult = Absent;
    pub type Ieee802154SessionStopEvidence = Absent;
    pub type Ieee802154SessionTransmitEvidence = Absent;
    pub type Ieee802154SessionTransmitRequest = Absent;
    pub type Ieee802154ThreadReceiveEvidence = Absent;
    pub type Ieee802154ThreadSendRequest = Absent;
    pub type Ieee802154ThreadStartRequest = Absent;
    pub type Ieee802154ThreadState = Absent;
}

#[cfg(not(feature = "system"))]
mod system {
    use super::Absent;

    pub type MemoryBenchmarkEvidence = Absent;
    pub type MemoryBenchmarkRequest = Absent;
}
