//! The rates of every frame one interface sends: a sans-IO machine above
//! the lower-MAC port ([owner decision on #563]).
//!
//! The service asks it for the retry plan of each frame by what the frame
//! is for ([`FrameRates`]) and hands it every terminal of those frames
//! ([`RateSelection::completed`]): the rates and transmissions the attempt
//! made teach it the next plan. It hears when its interface associates
//! with a peer and when it leaves: a controller adapts per peer. The selection is an integrator's policy;
//! the Espressif one (`oer-espressif-ieee80211-policy::rate_selection`)
//! sends as the vendor's station does.
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6132066470

use oer_ieee80211_lower_mac::{RetryPlan, TxCompletion};
use oer_ieee80211_mac::phy::PhyRate;
use oer_time::Instant;

use crate::rate_control::{LinkMetric, RatePeer};

/// What a frame is for, which picks its rates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameRates {
    /// A group Probe Request: one transmission, the first stage's rate.
    Probe,
    /// An individual frame that carries no network data: management,
    /// EAPOL, a Null, a BlockAckReq.
    Control,
    /// A data MPDU of the network, sent alone.
    Data,
}

/// The rates of one interface's frames.
pub trait RateSelection {
    /// The retry plan of the next frame for `frame`.
    fn plan(&self, frame: FrameRates) -> RetryPlan<PhyRate>;

    /// The interface associated with `peer`, whose signal over the noise
    /// floor was `link_metric` in the frame that made the link: from now on
    /// its frames go at the association's rates.
    fn associated(&mut self, peer: &RatePeer, link_metric: LinkMetric);

    /// The interface left its peer.
    fn disconnected(&mut self);

    /// A frame for `frame` ended at `now` with `completion`: its
    /// transmissions, the rate of the last, whether it was acknowledged.
    fn completed(&mut self, now: Instant, frame: FrameRates, completion: &TxCompletion);
}
