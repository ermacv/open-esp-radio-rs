//! Affine product of one radio phase and one independent HCI-order phase.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerEndpointMismatch, LeControllerResponsePending, LeControllerResponsePublication,
};

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

impl<'runtime, Phase>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        Phase,
        LeControllerResponsePending<'runtime, ()>,
    >
{
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.order).await
    }

    /// Attempt publication without consuming or pausing the recurrence phase.
    pub fn try_publish_response_with<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
        R,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
        published: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                Phase,
                LeControllerCommandReady<'runtime, ()>,
            >,
        ) -> R,
        pending: impl FnOnce(Self) -> R,
        endpoint_mismatch: impl FnOnce(Self) -> R,
        fault: impl FnOnce(Self, HciChannelError) -> R,
    ) -> R {
        match self
            .order
            .map_owner(|()| self.phase)
            .try_publish(controller)
        {
            LeControllerResponsePublication::Published(ordered) => {
                let (phase, order) = ordered.into_parts();
                published(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                let (phase, response) = transaction.into_parts();
                pending(Self::from_parts(phase, response))
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                let (phase, response) = transaction.into_parts();
                endpoint_mismatch(Self::from_parts(phase, response))
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => {
                let (phase, response) = transaction.into_parts();
                fault(Self::from_parts(phase, response), error)
            }
        }
    }
}

#[cfg(test)]
mod tests;
