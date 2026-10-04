//! The station's choice of its data rates, and what its exchanges teach it.
//!
//! Which rate a data frame is first sent at is the station's decision above
//! the lower-MAC port. [`StaRateControl`] is the seam: the station asks one
//! controller per association for the rate of a single MPDU and of an
//! A-MPDU, and reports every exchange's outcome to it. The controller is an
//! integrator's policy: [`StaFixedRateControl`] keeps one rate; the
//! Espressif controller of `oer-espressif-ieee80211-policy::rate_control`
//! adapts as the vendor station does.

use oer_ieee80211_mac::phy::PhyRate;
use oer_time::Instant;

use crate::association::StaAssociatedPeer;

/// One association's rate control.
pub trait StaRateControl: Sized {
    /// What the integrator configures every association's controller with.
    type Config: Copy;

    /// The controller of an association with `peer`. `link_metric` is the
    /// access point's signal over the noise floor, in dB, from its
    /// Association Response; `None` when the port reported either value
    /// unavailable.
    fn associate(config: Self::Config, peer: &StaAssociatedPeer, link_metric: Option<i8>) -> Self;

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
pub struct StaFixedRateControl(PhyRate);

impl StaRateControl for StaFixedRateControl {
    type Config = PhyRate;

    fn associate(rate: PhyRate, _peer: &StaAssociatedPeer, _link_metric: Option<i8>) -> Self {
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
