//! Chip-private composition of restored lifecycle ownership with portable Reset order.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    InProcessHciControllerEndpoint, LeControllerCommandEndpoint, LeControllerResetBarrier,
    LeControllerResetCompletion, LeControllerResponsePending,
};

pub(crate) struct BluetoothDtmRestoredReset<'epoch, Owner> {
    barrier: LeControllerResetBarrier<'epoch, Owner>,
}

pub(crate) enum BluetoothDtmRestoredResetCompletion<'epoch, Owner> {
    ResponsePending(LeControllerResponsePending<'epoch, Owner>),
    EndpointMismatch(BluetoothDtmRestoredReset<'epoch, Owner>),
}

impl<'epoch, Owner> BluetoothDtmRestoredReset<'epoch, Owner> {
    pub(crate) const fn new(barrier: LeControllerResetBarrier<'epoch, Owner>) -> Self {
        Self { barrier }
    }

    pub(crate) fn matches_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &InProcessHciControllerEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.barrier.accepts_endpoint(controller)
    }

    pub(crate) fn complete<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothDtmRestoredResetCompletion<'epoch, Owner> {
        match controller.complete_reset_after_quiescence(self.barrier) {
            LeControllerResetCompletion::ResponsePending(pending) => {
                BluetoothDtmRestoredResetCompletion::ResponsePending(pending)
            }
            LeControllerResetCompletion::EndpointMismatch(barrier) => {
                BluetoothDtmRestoredResetCompletion::EndpointMismatch(Self { barrier })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use open_esp_radio_bluetooth_hci::{
        BluetoothPublicDeviceAddress, BootstrapPhase, LE_RECEIVER_TEST_V1_OPCODE,
        LeControllerBootstrapConfig, LeControllerClassifiedCommandRoute, LeControllerHciResources,
        LeControllerResponsePending, LeControllerResponsePublication, LeDtmCommand,
        bt_hci::{cmd::controller_baseband::Reset, transport::Transport},
    };

    use super::{BluetoothDtmRestoredReset, BluetoothDtmRestoredResetCompletion};

    #[derive(Debug, Eq, PartialEq)]
    struct RestoredOwner(u32);

    type Resources = LeControllerHciResources<NoopRawMutex, 1, 1, 16>;

    fn resources() -> Resources {
        Resources::new(
            LeControllerBootstrapConfig::new(
                BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
                12,
                1,
            )
            .expect("the test profile is nonzero"),
        )
        .expect("the test profile fits its queues")
    }

    fn started_response() -> open_esp_radio_bluetooth_hci::LeDtmCommandCompleteEvent {
        let LeDtmCommand::ReceiverTestV1(command) =
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[7])
                .expect("the reviewed receiver command is valid")
        else {
            panic!("the receiver opcode changed command kind");
        };
        command.into_started_command_complete()
    }

    #[test]
    fn restored_reset_retains_affinity_and_response_through_backpressure() {
        let mut first_resources = resources();
        let mut first = first_resources.split();
        let start = LeControllerResponsePending::new(
            (),
            started_response(),
            first.controller.transport().epoch_identity(),
        );
        let LeControllerResponsePublication::Published(published) =
            start.try_publish(first.controller.transport())
        else {
            panic!("the empty response queue accepts start");
        };
        block_on(first.host.write(&Reset::new())).expect("Reset enters its origin queue");
        let mut command_buffer = [0; 16];
        let classified = first
            .controller
            .transport()
            .try_receive_classified_command(&mut command_buffer)
            .unwrap_or_else(|_| panic!("the origin endpoint classifies Reset"));
        let LeControllerClassifiedCommandRoute::ResetBarrier(barrier) = first
            .controller
            .route_classified_command(published, classified)
        else {
            panic!("active Reset becomes a lifecycle barrier");
        };
        let restored = BluetoothDtmRestoredReset::new(barrier.map_owner(|()| RestoredOwner(41)));

        let mut foreign_resources = resources();
        let mut foreign = foreign_resources.split();
        let BluetoothDtmRestoredResetCompletion::EndpointMismatch(restored) =
            restored.complete(&mut foreign.controller)
        else {
            panic!("a foreign endpoint retains the complete restored Reset");
        };
        assert!(restored.matches_endpoint(first.controller.transport()));
        assert_eq!(
            first.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        assert_eq!(
            foreign.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );

        let BluetoothDtmRestoredResetCompletion::ResponsePending(pending) =
            restored.complete(&mut first.controller)
        else {
            panic!("the origin endpoint applies restored Reset");
        };
        assert_eq!(pending.owner(), &RestoredOwner(41));
        assert_eq!(
            first.controller.bootstrap_phase(),
            BootstrapPhase::Configuring
        );
        let LeControllerResponsePublication::Pending(pending) =
            pending.try_publish(first.controller.transport())
        else {
            panic!("the queued start response backpressures Reset completion");
        };
        assert_eq!(pending.owner(), &RestoredOwner(41));

        let mut response_buffer = [0; 16];
        block_on(first.host.read(&mut response_buffer)).expect("Host drains start response");
        let LeControllerResponsePublication::Published(published) =
            pending.try_publish(first.controller.transport())
        else {
            panic!("Reset completion publishes after capacity returns");
        };
        assert_eq!(published.into_owner(), RestoredOwner(41));
        block_on(first.host.read(&mut response_buffer)).expect("Host receives Reset completion");
    }
}
