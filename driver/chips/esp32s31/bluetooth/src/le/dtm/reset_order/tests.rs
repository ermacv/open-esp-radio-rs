use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_bluetooth_hci::{
    BluetoothPublicDeviceAddress, BootstrapPhase, LeControllerBootstrapConfig,
    LeControllerClassifiedCommandRoute, LeControllerCommandIntake, LeControllerCommandReadyClaim,
    LeControllerHciResources, LeControllerIdleClassifiedCommandRoute,
    LeControllerResponsePublication,
    bt_hci::{
        ControllerToHostPacket,
        cmd::{controller_baseband::Reset, le::LeTestEnd},
        transport::Transport,
    },
};

use super::{BluetoothDtmRestoredReset, BluetoothDtmRestoredResetCompletion};

#[derive(Debug, Eq, PartialEq)]
struct RestoredOwner(u32);

type Resources = LeControllerHciResources<NoopRawMutex, 1, 1, 45>;

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

#[test]
fn restored_reset_retains_affinity_and_response_through_backpressure() {
    let mut first_resources = resources();
    let mut first = first_resources.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        first.controller.claim_initial_command_ready(())
    else {
        panic!("the epoch exposes its sole initial command authority");
    };
    block_on(first.host.write(&LeTestEnd::new())).expect("idle Test End enters its origin queue");
    let mut command_buffer = [0; 45];
    let LeControllerCommandIntake::Command { command, .. } = first
        .controller
        .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
    else {
        panic!("the origin endpoint consumes idle Test End");
    };
    let LeControllerIdleClassifiedCommandRoute::ResponsePending(filler) =
        first.controller.route_idle_classified_command(command)
    else {
        panic!("idle Test End creates its zero-count response");
    };
    let LeControllerResponsePublication::Published(ready) = filler.try_publish(&first.controller)
    else {
        panic!("the output queue starts empty");
    };

    block_on(first.host.write(&Reset::new())).expect("Reset enters its origin queue");
    let LeControllerCommandIntake::Command { command, .. } = first
        .controller
        .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
    else {
        panic!("the origin endpoint consumes Reset");
    };
    let LeControllerClassifiedCommandRoute::ResetBarrier(barrier) =
        first.controller.route_classified_command(command)
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
    assert!(restored.matches_endpoint(&first.controller));
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
    let LeControllerResponsePublication::Pending(pending) = pending.try_publish(&first.controller)
    else {
        panic!("the occupied output queue backpressures Reset completion");
    };
    assert_eq!(pending.owner(), &RestoredOwner(41));

    let mut response_buffer = [0; 45];
    block_on(
        first
            .host
            .read::<ControllerToHostPacket<'_>>(&mut response_buffer),
    )
    .expect("Host drains the preceding event");
    let LeControllerResponsePublication::Published(published) =
        pending.try_publish(&first.controller)
    else {
        panic!("Reset completion publishes after capacity returns");
    };
    let (owner, ready) = published.into_parts();
    assert_eq!(owner, RestoredOwner(41));
    assert!(ready.accepts_endpoint(&first.controller));
    block_on(
        first
            .host
            .read::<ControllerToHostPacket<'_>>(&mut response_buffer),
    )
    .expect("Host receives Reset completion");
}
