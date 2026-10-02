//! The station over the IEEE 802.11 lower-MAC port.
//!
//! Every station phase runs here over any
//! [`Ieee80211LowerMacPort`](oer_ieee80211_lower_mac::Ieee80211LowerMacPort),
//! without chip types: the backends of this crate's drivers and of the WPA2
//! runners of `oer-ieee80211-rsn-service`, the connected data plane and
//! power save. Transmission goes through the transmit planner of
//! `oer-ieee80211-upper-mac-service` ([`UpperMacTx`](oer_ieee80211_upper_mac_service::UpperMacTx)).
//!
//! | Item | Implements | Over the port |
//! | --- | --- | --- |
//! | [`PortLink`] | The single consumer of the port's events and its transmit driver | `next_event`, `submit` through `UpperMacTx`, `apply`, `lifecycle` |
//! | [`PortScan`] | `StaScanPort`, run by `StaScanBackend` and `StaCandidateScanService` | `Channel`, the `OTHER_BSS_MANAGEMENT` filter or `LowerMacMonitor`, Probe Requests |
//! | [`PortJoin`] | `StaJoinBackend`, run by `StaJoinRunner` | Open System and SAE Authentication, Association, the `BSS_MEMBER` filter |
//! | [`PortHandshake`], [`PortKeyInstall`] | `RsnHandshakeBackend`, `RsnKeyInstallBackend` | EAPOL frames, `install_key` of the pairwise and group keys |
//! | [`PortConnection`] | The connected data plane | QoS data with CCMP headers, receive reordering, replay and duplicate checks, A-MSDU, Block Ack agreements, SA Query, disconnection |
//! | [`PortPowerSave`] | Modem sleep of `oer_ieee80211_sta::modem_sleep` | `LowerMacBeaconTiming` TBTTs and TSF, `TxGate`, Null frames |
//! | [`PortStation`], [`PortAttemptPort`] | `StaAttemptPort`, run by `StaAttempt` | All of the above, in order |
//! | [`PortStationLifecycle`] | `StaLifecycleBackend`, run by `StaLifecycleService` | Attempts, the connection, backoff |
//!
//! The station consumes the port from one task: [`PortLink`] owns
//! `next_event` and the transmit driver, and every phase takes its inputs
//! from it in turn, so no two consumers compete for the port's events.

mod connected;
mod join;
mod link;
mod power;
mod rsn;
mod scan;
mod station;
mod wire;

pub use connected::{
    PORT_REORDER_SLOTS, PORT_REORDER_WINDOW, PortConnection, PortConnectionConfig, PortDisconnect,
    PortRxCounters, PortSend,
};
pub use join::{PortAssociation, PortJoin};
pub use link::{
    BeaconTimingOps, PORT_BACKLOG, PORT_FRAME_CAPACITY, PortError, PortFrame, PortInput, PortLink,
    PortLinkCounters, PortLinkError, PortStationConfig, PortStationEnv,
};
pub use power::PortPowerSave;
pub use rsn::{PortHandshake, PortKeyInstall, PortKeys};
pub use scan::{PortProbe, PortScan, PortScanTarget};
pub use station::{
    PortAttemptError, PortAttemptPort, PortAttemptReport, PortStation, PortStationApplication,
    PortStationError, PortStationLifecycle, PortStationProfile, PortUnwrapError,
};
