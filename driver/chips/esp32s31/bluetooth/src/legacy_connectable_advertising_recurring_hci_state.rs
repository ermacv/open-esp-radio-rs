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
mod tests {
    use core::{cell::Cell, future::Future, pin::pin, task::Context};
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerCommandIntake,
        LeControllerCommandReady, LeControllerCommandReadyClaim, LeControllerHciEndpoints,
        LeControllerHciResources, LeControllerIdleClassifiedCommandRoute,
        LeControllerResponsePending, LeControllerResponsePublication,
        bt_hci::{
            ControllerToHostPacket, FromHciBytes,
            cmd::le::LeTestEnd,
            event::{CommandComplete, CommandCompleteWithStatus},
            param::Status,
            transport::Transport,
        },
    };
    use std::task::Waker;

    use super::BluetoothLegacyConnectableAdvertisingRecurringHci as Recurring;

    type Resources = LeControllerHciResources<NoopRawMutex, 1, 1, 45>;
    type Endpoints<'epoch> = LeControllerHciEndpoints<'epoch, NoopRawMutex, 1, 1, 45>;

    // Only the radio phase is replaced. HCI authority, classification,
    // publication and queue capacity all use the production implementation.
    struct RadioPhase<'a> {
        progress: &'a Cell<usize>,
        drops: &'a Cell<usize>,
    }

    impl Drop for RadioPhase<'_> {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    fn resources() -> Resources {
        Resources::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                12,
                1,
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn claim<'epoch>(endpoints: &mut Endpoints<'epoch>) -> LeControllerCommandReady<'epoch, ()> {
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the epoch owns its initial command authority");
        };
        ready
    }

    fn response<'epoch>(
        endpoints: &mut Endpoints<'epoch>,
        ready: LeControllerCommandReady<'epoch, ()>,
    ) -> LeControllerResponsePending<'epoch, ()> {
        block_on(endpoints.host.write(&LeTestEnd::new())).unwrap();
        let mut buffer = [0; 45];
        let LeControllerCommandIntake::Command { command, .. } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(ready, &mut buffer)
        else {
            panic!("the endpoint classifies the queued command");
        };
        let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("idle Test End produces an immediate response");
        };
        pending
    }

    fn read_response(endpoints: &Endpoints<'_>) {
        let mut buffer = [0; 45];
        let ControllerToHostPacket::Event(event) =
            block_on(endpoints.host.read(&mut buffer)).unwrap()
        else {
            panic!("the response is an HCI event");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
        let complete: CommandCompleteWithStatus<'_> = complete.try_into().unwrap();
        assert_eq!(complete.status, Status::SUCCESS);
        assert_eq!(complete.return_params::<LeTestEnd>().unwrap(), 0);
    }

    fn assert_queue_empty(endpoints: &Endpoints<'_>) {
        let mut buffer = [0; 45];
        let mut read = pin!(
            endpoints
                .host
                .read::<ControllerToHostPacket<'_>>(&mut buffer)
        );
        assert!(
            read.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }

    #[test]
    fn full_queue_retains_response_and_allows_same_radio_phase_to_progress() {
        let mut resources = resources();
        let mut endpoints = resources.split();
        let ready = claim(&mut endpoints);
        let LeControllerResponsePublication::Published(ready) =
            response(&mut endpoints, ready).try_publish(&endpoints.controller)
        else {
            panic!("the first response fills the output queue");
        };
        let progress = Cell::new(0);
        let drops = Cell::new(0);
        let mut recurring = Recurring::from_parts(
            RadioPhase {
                progress: &progress,
                drops: &drops,
            },
            response(&mut endpoints, ready),
        );
        {
            let mut wait = pin!(recurring.wait_response_capacity(&endpoints.controller));
            assert!(
                wait.as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        for expected in 1..=2 {
            recurring = recurring.try_publish_response_with(
                &endpoints.controller,
                |_| panic!("the older response still occupies the queue"),
                |state| {
                    state.phase.progress.set(state.phase.progress.get() + 1);
                    state
                },
                |_| panic!("the origin endpoint matches"),
                |_, error| panic!("unexpected publication fault: {error:?}"),
            );
            assert_eq!(progress.get(), expected);
            assert_eq!(drops.get(), 0);
        }
        read_response(&endpoints);
        block_on(recurring.wait_response_capacity(&endpoints.controller)).unwrap();
        let ready = recurring.try_publish_response_with(
            &endpoints.controller,
            |state| state,
            |_| panic!("the output queue has capacity"),
            |_| panic!("the origin endpoint matches"),
            |_, error| panic!("unexpected publication fault: {error:?}"),
        );
        assert!(core::ptr::eq(ready.phase.progress, &progress));
        assert_eq!(ready.phase.progress.get(), 2);
        assert_eq!(drops.get(), 0);
        assert!(ready.order.accepts_endpoint(&endpoints.controller));
        read_response(&endpoints);
        assert_queue_empty(&endpoints);
        drop(ready);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn foreign_endpoint_retains_radio_and_response_for_origin_publication() {
        let mut resources = resources();
        let mut endpoints = resources.split();
        let mut foreign_resources = self::resources();
        let foreign = foreign_resources.split();
        let ready = claim(&mut endpoints);
        let progress = Cell::new(7);
        let drops = Cell::new(0);
        let recurring = Recurring::from_parts(
            RadioPhase {
                progress: &progress,
                drops: &drops,
            },
            response(&mut endpoints, ready),
        );
        let recurring = recurring.try_publish_response_with(
            &foreign.controller,
            |_| panic!("a foreign endpoint cannot publish"),
            |_| panic!("affinity is checked before capacity"),
            |state| state,
            |_, error| panic!("unexpected publication fault: {error:?}"),
        );
        assert!(core::ptr::eq(recurring.phase.progress, &progress));
        assert_eq!(progress.get(), 7);
        assert_eq!(drops.get(), 0);
        assert_queue_empty(&endpoints);
        assert_queue_empty(&foreign);
        let ready = recurring.try_publish_response_with(
            &endpoints.controller,
            |state| state,
            |_| panic!("the output queue has capacity"),
            |_| panic!("the response retains origin affinity"),
            |_, error| panic!("unexpected publication fault: {error:?}"),
        );
        assert!(core::ptr::eq(ready.phase.progress, &progress));
        read_response(&endpoints);
        assert_queue_empty(&endpoints);
        assert_queue_empty(&foreign);
        drop(ready);
        assert_eq!(drops.get(), 1);
    }
}
