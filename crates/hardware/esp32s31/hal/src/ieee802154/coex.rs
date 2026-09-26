//! Software-coexistence priorities of the IEEE 802.15.4 MAC.
//!
//! In a build with software coexistence the pinned driver publishes the MAC's
//! two PTI fields through libcoexist: `ieee802154_mac_init` sets the ACK PTI
//! to the middle level and the TX/RX PTI to the idle scene, and each
//! operation start switches the TX/RX PTI to its scene
//! (`ieee802154_set_txrx_pti` of `esp_ieee802154_util.c`, ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`). The priority of a level is
//! read from the arbiter's shared table ([`CoexPtiTable::ieee802154_pti`]).
//! A build without software coexistence disables both fields instead
//! (`ieee802154_ll_disable_coex`).

use crate::coex::{CoexPti, CoexPtiTable, Ieee802154CoexLevel};

/// Operation scene of the TX/RX PTI (`ieee802154_txrx_scene_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154CoexScene {
    /// After MAC initialization.
    Idle,
    /// Immediate transmission.
    Tx,
    /// Reception, energy detection and CCA.
    Rx,
    /// Timed transmission.
    TxAt,
    /// Timed reception.
    RxAt,
}

/// Level of each TX/RX scene (`esp_ieee802154_coex_config_t`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154CoexConfig {
    /// Level of the idle scene.
    pub idle: Ieee802154CoexLevel,
    /// Level of immediate transmission and reception.
    pub txrx: Ieee802154CoexLevel,
    /// Level of timed transmission and reception.
    pub txrx_at: Ieee802154CoexLevel,
}

impl Ieee802154CoexConfig {
    /// The driver's default `s_coex_config`.
    pub const VENDOR: Self = Self {
        idle: Ieee802154CoexLevel::Idle,
        txrx: Ieee802154CoexLevel::Low,
        txrx_at: Ieee802154CoexLevel::Middle,
    };
}

impl Default for Ieee802154CoexConfig {
    fn default() -> Self {
        Self::VENDOR
    }
}

/// Level `ieee802154_mac_init` gives the ACK PTI.
pub const IEEE802154_ACK_COEX_LEVEL: Ieee802154CoexLevel = Ieee802154CoexLevel::Middle;

/// The priorities of every scene and of the ACK, resolved from one snapshot
/// of the shared table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154CoexPriorities {
    idle: CoexPti,
    txrx: CoexPti,
    txrx_at: CoexPti,
    ack: CoexPti,
}

impl Ieee802154CoexPriorities {
    /// Resolve `config` and the ACK level against `table`.
    pub const fn resolve(config: Ieee802154CoexConfig, table: &CoexPtiTable) -> Self {
        Self {
            idle: table.ieee802154_pti(config.idle),
            txrx: table.ieee802154_pti(config.txrx),
            txrx_at: table.ieee802154_pti(config.txrx_at),
            ack: table.ieee802154_pti(IEEE802154_ACK_COEX_LEVEL),
        }
    }

    /// The TX/RX priority of `scene`.
    pub const fn scene(&self, scene: Ieee802154CoexScene) -> CoexPti {
        match scene {
            Ieee802154CoexScene::Idle => self.idle,
            Ieee802154CoexScene::Tx | Ieee802154CoexScene::Rx => self.txrx,
            Ieee802154CoexScene::TxAt | Ieee802154CoexScene::RxAt => self.txrx_at,
        }
    }

    /// The ACK priority.
    pub const fn ack(&self) -> CoexPti {
        self.ack
    }
}

/// How the MAC takes part in coexistence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154Coexistence {
    /// A build without software coexistence: both PTIs are disabled.
    #[default]
    Disabled,
    /// Software coexistence with these priorities.
    Software(Ieee802154CoexPriorities),
}

#[cfg(test)]
mod tests;
