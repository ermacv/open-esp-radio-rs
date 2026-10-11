//! The outcome of one shared PHY maintenance tick of the IEEE 802.15.4
//! client.

/// Outcome of one PHY maintenance attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PhyMaintenance {
    /// No tracking was due.
    NotDue,
    /// Tracking ran with IEEE 802.15.4 running.
    Tracked,
    /// Tracking is due, but the domain's maintenance policy waits for the
    /// quiescence of every active client, which one client cannot prove;
    /// the vendor policy never waits. IEEE 802.15.4 keeps running.
    AwaitingOtherClients,
}
