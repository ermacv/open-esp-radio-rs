#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent Wi-Fi access point over any lower-MAC port.
//!
//! [`port`] runs the access point over the port's
//! [`PortClient`](oer_ieee80211_upper_mac_service::client::PortClient): it
//! starts the BSS, publishes its beacons at the TBTTs of its own schedule
//! and answers Probe Requests. The policy (peers, security, power save)
//! belongs to `oer-ieee80211-ap`; frame codecs to `oer-ieee80211-mac`.

pub mod port;
