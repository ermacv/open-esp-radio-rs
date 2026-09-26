//! When the IEEE 802.15.4 client asks for shared PHY maintenance again.

use oer_esp32s31_phy::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS;

/// Outcome of one PHY maintenance attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PhyMaintenance {
    /// No tracking was due.
    NotDue,
    /// Tracking ran: under the vendor policy with IEEE 802.15.4 running,
    /// under the quiesced policy inside its quiescence window.
    Tracked,
    /// Under the quiesced policy tracking is due, but another client is
    /// active: the maintenance must collect every active client's quiescence
    /// proof, which one client cannot. IEEE 802.15.4 keeps running.
    AwaitingOtherClients,
    /// Under the quiesced policy a transmission, energy scan or CCA is
    /// running; retry after it ends.
    Busy,
}

/// Evaluation period: the shared PHY domain's tracking period, which is
/// ESP-IDF's `CONFIG_ESP_PHY_PLL_TRACK_PERIOD_MS` default.
pub const MAINTENANCE_PERIOD_MICROS: u64 = DEFAULT_PLL_TRACK_PERIOD_MICROS;

/// Retry delay while an operation keeps the MAC busy. Operations end within
/// milliseconds, far inside one tracking period.
pub const BUSY_RETRY_MICROS: u64 = 1_000;

const _: () = assert!(BUSY_RETRY_MICROS < MAINTENANCE_PERIOD_MICROS);

/// Delay before the next maintenance attempt after `outcome`.
///
/// Due tracking that the MAC defers is retried promptly, so an operation in
/// flight postpones it by at most one operation; every other outcome waits
/// one period, as the vendor's periodic timer does.
pub const fn next_attempt_micros(outcome: Ieee802154PhyMaintenance) -> u64 {
    match outcome {
        Ieee802154PhyMaintenance::Busy => BUSY_RETRY_MICROS,
        Ieee802154PhyMaintenance::NotDue
        | Ieee802154PhyMaintenance::Tracked
        | Ieee802154PhyMaintenance::AwaitingOtherClients => MAINTENANCE_PERIOD_MICROS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_busy_mac_retries_before_the_next_period() {
        assert_eq!(
            next_attempt_micros(Ieee802154PhyMaintenance::Busy),
            BUSY_RETRY_MICROS
        );
        for outcome in [
            Ieee802154PhyMaintenance::NotDue,
            Ieee802154PhyMaintenance::Tracked,
            Ieee802154PhyMaintenance::AwaitingOtherClients,
        ] {
            assert_eq!(next_attempt_micros(outcome), MAINTENANCE_PERIOD_MICROS);
        }
    }
}
