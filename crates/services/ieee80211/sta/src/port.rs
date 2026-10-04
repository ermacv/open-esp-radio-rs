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
//! | [`PortLink`] | The station's client of the port's [`PortRouter`] and its transmit driver | The router's receive, extension and lifecycle queues, `submit` through `UpperMacTx`, `apply`, `lifecycle`, `cancel` after a loss |
//! | [`PortScan`] | `StaScanPort`, run by `StaScanBackend` and `StaCandidateScanService` | `Channel`, the `OTHER_BSS_MANAGEMENT` filter or `LowerMacMonitor`, Probe Requests |
//! | [`PortJoin`] | `StaJoinBackend`, run by `StaJoinRunner` | Open System and SAE Authentication, Association, the `BSS_MEMBER` filter |
//! | [`PortHandshake`], [`PortKeyInstall`] | `RsnHandshakeBackend`, `RsnKeyInstallBackend` | EAPOL frames, `install_key` of the pairwise and group keys |
//! | [`PortConnection`] | The connected data plane | a transmit queue sent as MPDUs or A-MPDUs, QoS data with CCMP headers, TX Block Ack agreements, receive reordering, replay and duplicate checks, A-MSDU, Block Ack agreements, SA Query, group rekeys, disconnection |
//! | [`PortPowerSave`] | Modem sleep of `oer_ieee80211_sta::modem_sleep` | `LowerMacBeaconTiming` TBTTs and TSF, `TxGate`, Null frames |
//! | [`PortStation`], [`PortAttemptPort`] | `StaAttemptPort`, run by `StaAttempt` | All of the above, in order |
//! | [`PortStationLifecycle`] | `StaLifecycleBackend`, run by `StaLifecycleService` | Attempts, the connection, backoff |
//!
//! The port's one event consumer is its [`PortRouter`]
//! (`oer-ieee80211-upper-mac-service`'s `EventRouter`), which the
//! composition polls beside the station; the station runs in one task and
//! [`PortLink`] reads the router's queues and transmits through it, so no
//! two consumers compete for the port's events. A loss is an
//! [`PortInput::EventsLost`] input, and an exchange whose completion was in
//! the gap cancels its attempt; the terminal poisoned event ends every phase
//! with [`PortLinkError::Poisoned`].

mod connected;
mod join;
mod link;
mod power;
mod rsn;
mod scan;
mod station;
mod wire;

pub use connected::{
    PORT_REORDER_SLOTS, PORT_REORDER_WINDOW, PortConnection, PortConnectionBuffers,
    PortConnectionConfig, PortDisconnect, PortRxCounters, PortSend, PortTxCounters,
};
pub use join::{PortAssociation, PortHePower, PortJoin};
pub use link::{
    EventRouter, NoAggregation, NoCoexistence, PORT_BACKLOG, PORT_EXCHANGES, PortAggregation,
    PortAmpduAggregation, PortCoexistence, PortCoexistenceRefused, PortConnectionFrame, PortLink,
    PortLinkError, PortRouter, PortStationConfig, PortStationEnv,
};
pub use power::PortPowerSave;
pub use rsn::{PortHandshake, PortKeyInstall, PortKeys};
pub use scan::{PortProbe, PortScan, PortScanTarget};
pub use station::{
    LinkSupervisionError, PortAttemptError, PortAttemptPort, PortAttemptReport,
    PortLinkSupervision, PortStation, PortStationApplication, PortStationError,
    PortStationLifecycle, PortStationProfile, PortStationStorage, PortTxBlockAck, PortUnwrapError,
};
