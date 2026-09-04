//! Affine product of one radio phase and one independent HCI-order phase.

#![forbid(unsafe_code)]

/// One exact recurrence phase paired with one independent HCI-order axis.
#[must_use = "advance both the recurrence and HCI-order axes"]
pub struct BluetoothLegacyConnectableAdvertisingRecurringHci<Phase, Order> {
    pub(crate) phase: Phase,
    pub(crate) order: Order,
}

impl<Phase, Order> BluetoothLegacyConnectableAdvertisingRecurringHci<Phase, Order> {
    pub(crate) const fn from_parts(phase: Phase, order: Order) -> Self {
        Self { phase, order }
    }
}
