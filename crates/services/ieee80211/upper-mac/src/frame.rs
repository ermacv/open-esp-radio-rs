//! The frame sizes a service keeps.
//!
//! A service does not queue the frames it sends: it takes the network's own
//! owners from its source (`oer-ieee80211-datapath`'s `DestinationTxQueues`)
//! when it is ready to send them, and keeps an owner, not a copy, while a
//! frame waits for a dozing peer.

use oer_ieee80211_datapath::SoftwareTxFrame;
use oer_ieee80211_lower_mac::TxBody;
use oer_ieee80211_mac::data::ETHERNET_HEADER_LEN;

/// Octets of one frame a service keeps: a received MPDU, or an Ethernet
/// frame it sends.
pub const PORT_FRAME_CAPACITY: usize = 2_352;

/// Octets of the header a service encodes before an Ethernet frame's
/// payload: its MAC, QoS and CCMP headers, LLC/SNAP and EtherType.
pub const PORT_MPDU_HEADER_CAPACITY: usize = 64;

/// Octets of one MPDU a service encodes from an Ethernet frame: the frame,
/// its MAC, QoS and CCMP headers and LLC/SNAP.
pub const PORT_MPDU_CAPACITY: usize = PORT_FRAME_CAPACITY + PORT_MPDU_HEADER_CAPACITY;

/// The Ethernet header of the Ethernet-II frame `ethernet`, which a service
/// encodes into an MPDU's header, and its payload, which the MPDU carries
/// unchanged after it. A frame shorter than a header is all header, so the
/// encoder refuses it.
pub fn split_ethernet(ethernet: &[u8]) -> (&[u8], &[u8]) {
    ethernet.split_at(ethernet.len().min(ETHERNET_HEADER_LEN))
}

/// A network frame as the body of the MPDU that carries it: its payload,
/// after the Ethernet header the service encodes into the MPDU's header.
/// The port holds it while an attempt sends it; dropping it hands the frame
/// back to the network.
#[derive(Debug, Eq, PartialEq)]
pub struct NetworkBody<F>(pub F);

impl<F: SoftwareTxFrame> TxBody for NetworkBody<F> {
    fn bytes(&self) -> &[u8] {
        split_ethernet(self.0.ethernet()).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ethernet_frame_splits_after_its_header() {
        let frame = [7_u8; 20];
        assert_eq!(split_ethernet(&frame), (&frame[..14], &frame[14..]));
        assert_eq!(split_ethernet(&frame[..9]), (&frame[..9], &[][..]));
    }
}
