//! The uncoded PHYs of a connection and the preferences of the PHY Update
//! procedure.
//!
//! PHY masks carry bit 0 for LE 1M, bit 1 for LE 2M and bit 2 for LE Coded
//! (Core Vol 6, Part B, 2.4.2.22). This Link Layer runs connections on LE 1M
//! and LE 2M only; LE Coded is never offered or accepted.

pub use oer_bluetooth_radio::{ConnectionPhy, ConnectionPhys};

/// The bit of `phy` in a PHY mask.
pub const fn phy_mask(phy: ConnectionPhy) -> u8 {
    match phy {
        ConnectionPhy::Le1M => 1,
        ConnectionPhy::Le2M => 2,
    }
}

/// The PHY of a mask with exactly one supported bit.
pub const fn phy_from_single_mask(mask: u8) -> Option<ConnectionPhy> {
    match mask {
        1 => Some(ConnectionPhy::Le1M),
        2 => Some(ConnectionPhy::Le2M),
        _ => None,
    }
}

/// PHYs supported by this Link Layer, as a PHY mask.
pub const LE_SUPPORTED_PHYS: u8 = 0b11;

/// The PHYs one side prefers for each direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePhyPreference {
    transmit: u8,
    receive: u8,
}

impl LePhyPreference {
    /// Every supported PHY in both directions.
    pub const ANY: Self = Self {
        transmit: LE_SUPPORTED_PHYS,
        receive: LE_SUPPORTED_PHYS,
    };

    /// A preference for the supported PHYs of `transmit` and `receive`.
    /// An empty direction, or one without a supported PHY, prefers every
    /// supported PHY.
    pub const fn new(transmit: u8, receive: u8) -> Self {
        const fn supported(mask: u8) -> u8 {
            match mask & LE_SUPPORTED_PHYS {
                0 => LE_SUPPORTED_PHYS,
                mask => mask,
            }
        }
        Self {
            transmit: supported(transmit),
            receive: supported(receive),
        }
    }

    /// Preferred transmit PHYs.
    pub const fn transmit(self) -> u8 {
        self.transmit
    }

    /// Preferred receive PHYs.
    pub const fn receive(self) -> u8 {
        self.receive
    }
}

/// A PHY change the Central sent in `LL_PHY_UPDATE_IND`, from the
/// Peripheral's side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePhyTransition {
    previous: ConnectionPhys,
    updated: ConnectionPhys,
}

impl LePhyTransition {
    pub(crate) const fn new(previous: ConnectionPhys, updated: ConnectionPhys) -> Self {
        Self { previous, updated }
    }

    /// The PHYs before the instant.
    pub const fn previous(self) -> ConnectionPhys {
        self.previous
    }

    /// The PHYs from the instant on.
    pub const fn updated(self) -> ConnectionPhys {
        self.updated
    }

    /// Whether either direction changed.
    pub const fn changed(self) -> bool {
        self.previous.transmit as u8 != self.updated.transmit as u8
            || self.previous.receive as u8 != self.updated.receive as u8
    }
}

#[cfg(test)]
mod tests;
