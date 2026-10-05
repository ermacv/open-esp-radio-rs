//! The frame sizes a service keeps.
//!
//! A service does not queue the frames it sends: it takes the network's own
//! owners from its source (`oer-ieee80211-datapath`'s `DestinationTxQueues`)
//! when it is ready to send them, and keeps an owner, not a copy, while a
//! frame waits for a dozing peer.

/// Octets of one frame a service keeps: a received MPDU, or an Ethernet
/// frame it sends.
pub const PORT_FRAME_CAPACITY: usize = 2_352;

/// Octets of one MPDU a service encodes from an Ethernet frame: the frame,
/// its MAC, QoS and CCMP headers and LLC/SNAP.
pub const PORT_MPDU_CAPACITY: usize = PORT_FRAME_CAPACITY + 64;
