//! The station's power-management state as its access point sees it: the
//! first decision of `modem_sleep`'s portable decision half ([owner
//! decision on #563]), which its split takes over without a second copy.
//!
//! A station tells its access point whether it sleeps with the PM bit of a
//! Null. A Null's retries are its retry plan in the driver
//! ([`NULL_TRANSMISSIONS`]); one the plan could not deliver leaves the two
//! sides apart. The decision here closes the gap the one way the station
//! can observe it: awake, it sent PM=0 that never arrived, so the access
//! point may still buffer its frames, and a beacon whose TIM shows the
//! station's association identifier says it does: the station sends PM=0
//! again. The beacon is read with
//! [`parse_sta_beacon`](oer_ieee80211_mac::station_beacon::parse_sta_beacon).
//!
//! [owner decision on #563]: https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130033259

use oer_ieee80211_mac::station_beacon::StaTimObservation;
use oer_ieee80211_mac::station_power_save::StaPowerManagement;

/// The transmissions a Null's retry plan makes: the vendor's limit of
/// Null retries, plus the first.
pub const NULL_TRANSMISSIONS: u8 = crate::modem_sleep::NULL_RETRY_LIMIT + 1;

/// What the station knows of its access point's view of its power state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PowerSaveView {
    /// The station's own state: it sleeps.
    asleep: bool,
    /// The access point may think the station sleeps: a PM=1 Null may have
    /// arrived and no PM=0 since.
    peer_may_think_asleep: bool,
}

/// What the station does about its access point's view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerSaveDecision {
    /// Nothing.
    Keep,
    /// Send a Null with PM=0 again: the access point buffers frames for an
    /// awake station.
    SendActive,
}

impl PowerSaveView {
    /// The view of an awake station whose access point thinks it awake.
    pub const fn awake() -> Self {
        Self {
            asleep: false,
            peer_may_think_asleep: false,
        }
    }

    /// The station sent a Null with `power_management`, which its retry
    /// plan delivered or not.
    pub fn null_sent(&mut self, power_management: StaPowerManagement, delivered: bool) {
        match power_management {
            StaPowerManagement::PowerSave => {
                self.asleep = true;
                // Even an unacknowledged PM=1 may have reached the access
                // point: its ACK alone may have been lost.
                self.peer_may_think_asleep = true;
            }
            StaPowerManagement::Active => {
                self.asleep = false;
                if delivered {
                    self.peer_may_think_asleep = false;
                }
            }
        }
    }

    /// A beacon of the station's BSS with `tim`: what the station does.
    pub fn beacon(&self, tim: Option<StaTimObservation>) -> PowerSaveDecision {
        match tim {
            Some(tim) if !self.asleep && self.peer_may_think_asleep && tim.unicast_buffered => {
                PowerSaveDecision::SendActive
            }
            _ => PowerSaveDecision::Keep,
        }
    }

    /// Whether the station sleeps.
    pub const fn asleep(&self) -> bool {
        self.asleep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tim(unicast_buffered: bool) -> Option<StaTimObservation> {
        Some(StaTimObservation {
            dtim_count: 0,
            dtim_period: 1,
            unicast_buffered,
            group_buffered: false,
        })
    }

    #[test]
    fn an_undelivered_active_null_is_sent_again_when_the_tim_shows_our_frames() {
        let mut view = PowerSaveView::awake();
        view.null_sent(StaPowerManagement::PowerSave, true);
        view.null_sent(StaPowerManagement::Active, false);
        // The access point buffers for the station: send PM=0 again.
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::SendActive);
        // A TIM without the station's bit says nothing.
        assert_eq!(view.beacon(tim(false)), PowerSaveDecision::Keep);
        // Delivered at last: the two sides agree.
        view.null_sent(StaPowerManagement::Active, true);
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::Keep);
    }

    #[test]
    fn a_sleeping_station_leaves_its_buffered_frames_to_its_wake() {
        let mut view = PowerSaveView::awake();
        view.null_sent(StaPowerManagement::PowerSave, false);
        assert!(view.asleep());
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::Keep);
    }
}
