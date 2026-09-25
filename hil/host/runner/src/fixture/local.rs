//! Laptop-owned Linux fixture: the managed AP, the controlled client, passive
//! air and wire capture, and the root-owned radio helper they share.
pub(crate) mod air_monitor;
pub(crate) mod ap;
pub(crate) mod client;
pub(crate) mod evidence;
pub(crate) mod network_helper;
pub(crate) mod wire_capture;
mod wpa_control;
