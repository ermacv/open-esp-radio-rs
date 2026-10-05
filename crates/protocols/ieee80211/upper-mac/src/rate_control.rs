//! The choice of data rates to one peer, and what its exchanges teach it.
//!
//! Which rate a data frame is first sent at is decided above the lower-MAC
//! port, for every role alike. [`RateControl`] is the seam: a station keeps
//! one controller for its access point, an access point one per associated
//! station; each asks its controller for the rate of a single MPDU and of an
//! A-MPDU and reports every exchange's outcome to it. The controller is an
//! integrator's policy: [`FixedRateControl`] keeps one rate; the Espressif
//! controller of `oer-espressif-ieee80211-policy::rate_control` adapts as the
//! vendor does.

use oer_ieee80211_lower_mac::{RxEvidence, RxMeta};
use oer_ieee80211_mac::{
    he::{He20Capabilities, He20PeerState},
    ht::HtPeerCapabilities,
    phy::PhyRate,
    station::association::PhyMode,
};
use oer_time::Instant;

/// What a controller knows of the peer it sends to: the PHY mode of the
/// link and the peer's receive capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RatePeer {
    /// The mode the link runs in: the peers' common format and width.
    pub phy: PhyMode,
    pub ht_capabilities: Option<HtPeerCapabilities>,
    pub he_capabilities: Option<He20Capabilities>,
    /// The HE state of an HE link.
    pub he_peer_state: Option<He20PeerState>,
}

/// The rate control of one peer.
pub trait RateControl: Sized {
    /// What the integrator configures every peer's controller with.
    type Config: Copy;

    /// The controller of `peer`. `link_metric` is the peer's signal over the
    /// noise floor, in dB, in the frame that made the link (the station's
    /// Association Response, or the access point's Association Request);
    /// `None` when the port reported either value unavailable.
    fn for_peer(config: Self::Config, peer: &RatePeer, link_metric: Option<i8>) -> Self;

    /// The rate of a data MPDU's first attempt sent alone.
    fn mpdu_rate(&self) -> PhyRate;

    /// The rate of an A-MPDU's first attempt.
    fn ampdu_rate(&self) -> PhyRate;

    /// A single data MPDU's exchange ended after `attempts` publications,
    /// `acknowledged` or not, with the ACK SNR of a valid sample.
    fn observe_mpdu(&mut self, attempts: u8, acknowledged: bool, ack_snr_db: Option<i8>);

    /// An A-MPDU exchange ended at `now` with `acknowledged` of its
    /// `attempted` subframes in its BlockAck, with the BlockAck's ACK SNR
    /// when the backend reported a valid one.
    fn observe_ampdu(
        &mut self,
        now: Instant,
        attempted: u16,
        acknowledged: u16,
        ack_snr_db: Option<i8>,
    );
}

/// Every data frame at one configured rate, whatever its exchanges show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedRateControl(PhyRate);

impl RateControl for FixedRateControl {
    type Config = PhyRate;

    fn for_peer(rate: PhyRate, _peer: &RatePeer, _link_metric: Option<i8>) -> Self {
        Self(rate)
    }

    fn mpdu_rate(&self) -> PhyRate {
        self.0
    }

    fn ampdu_rate(&self) -> PhyRate {
        self.0
    }

    fn observe_mpdu(&mut self, _attempts: u8, _acknowledged: bool, _ack_snr_db: Option<i8>) {}

    fn observe_ampdu(
        &mut self,
        _now: Instant,
        _attempted: u16,
        _acknowledged: u16,
        _ack_snr_db: Option<i8>,
    ) {
    }
}

/// The peer's signal over the noise floor in a frame's receive metadata,
/// narrowed to a signed byte as the vendor's `ic_set_trc` does; `None` when
/// the port reported either value unavailable.
pub fn link_metric(meta: RxMeta) -> Option<i8> {
    let value = |evidence: RxEvidence<i8>| match evidence {
        RxEvidence::HardwareObserved(value) | RxEvidence::ProtocolValidated(value) => Some(value),
        RxEvidence::Unavailable => None,
    };
    Some(value(meta.rssi_dbm)?.wrapping_sub(value(meta.noise_floor_dbm)?))
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_lower_mac::Channel;
    use oer_ieee80211_mac::{channel::WifiChannel, phy::LegacyRate};

    use super::*;

    #[test]
    fn the_link_metric_needs_both_signal_and_noise_floor() {
        let channel = Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap());
        let unavailable = RxMeta::unavailable(channel);
        assert_eq!(link_metric(unavailable), None);
        let observed = RxMeta {
            rssi_dbm: RxEvidence::HardwareObserved(-40),
            noise_floor_dbm: RxEvidence::ProtocolValidated(-96),
            ..unavailable
        };
        assert_eq!(link_metric(observed), Some(56));
        assert_eq!(
            link_metric(RxMeta {
                noise_floor_dbm: RxEvidence::Unavailable,
                ..observed
            }),
            None
        );
    }

    #[test]
    fn a_fixed_controller_keeps_its_rate() {
        let rate = PhyRate::Legacy(LegacyRate::Ofdm24M);
        let peer = RatePeer {
            phy: PhyMode::Legacy,
            ht_capabilities: None,
            he_capabilities: None,
            he_peer_state: None,
        };
        let mut control = FixedRateControl::for_peer(rate, &peer, Some(10));
        control.observe_mpdu(7, false, None);
        assert_eq!((control.mpdu_rate(), control.ampdu_rate()), (rate, rate));
    }
}
