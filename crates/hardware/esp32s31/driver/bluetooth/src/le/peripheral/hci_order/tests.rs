use core::cell::Cell;
use std::rc::Rc;

use embassy_futures::{
    block_on,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerCommandReadyClaim,
    LeControllerHciResources, LeHostCompletedPacketsErrorEvent,
    bt_hci::{ControllerToHostPacket, PacketKind, transport::Transport},
};

use super::{
    LegacyConnectablePeripheralFirstHciAxis as Axis,
    LegacyConnectablePeripheralFirstHciOrder as Order,
    LegacyConnectablePeripheralFirstHciOrderPublication as Publication,
    LegacyConnectablePeripheralFirstHciResponseWait as ResponseWait,
};

type Resources = LeControllerHciResources<NoopRawMutex, 1, 1, 80>;

fn resources() -> Resources {
    LeControllerHciResources::new(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            12,
            1,
        )
        .expect("the bounded HCI profile is valid"),
    )
    .expect("the real packet storage fits this profile")
}

struct LocalOwner {
    id: u32,
    dropped: Rc<Cell<usize>>,
}

impl Drop for LocalOwner {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

#[test]
fn pending_response_cancel_mismatch_and_backpressure_retain_one_local_owner() {
    let dropped = Rc::new(Cell::new(0));
    let mut first_storage = resources();
    let mut first = first_storage.split();
    let mut other_storage = resources();
    let other = other_storage.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        first.controller.claim_initial_command_ready(LocalOwner {
            id: 41,
            dropped: Rc::clone(&dropped),
        })
    else {
        panic!("the sole initial authority is unclaimed");
    };
    let pending =
        Order::ResponsePending(ready.begin_host_completed_packets_error(
            LeHostCompletedPacketsErrorEvent::invalid_parameters(),
        ));
    assert_eq!(pending.axis(), Axis::ResponsePending);
    assert!(!pending.accepts_endpoint(&other.controller));
    assert!(pending.accepts_endpoint(&first.controller));
    assert!(block_on(pending.wait_response_capacity(&other.controller)).is_err());

    let pending = match pending.try_publish_response(&other.controller) {
        Publication::EndpointMismatch(pending) => pending,
        Publication::Fault { order, error } => {
            drop(order);
            panic!("a foreign endpoint unexpectedly faulted: {error:?}");
        }
        _ => panic!("a foreign epoch cannot consume the pending owner or response"),
    };
    assert_eq!(pending.owner().id, 41);
    assert_eq!(dropped.get(), 0);

    let Publication::Published(ready) = pending.try_publish_response(&first.controller) else {
        panic!("the matching empty output queue accepts the first response");
    };
    assert_eq!(ready.axis(), Axis::CommandReady);
    assert_eq!(ready.owner().id, 41);
    let (owner, order) = ready.into_parts();
    let Order::CommandReady(unit_ready) = order else {
        panic!("publication restored the exact command-ready epoch");
    };
    let pending = Order::ResponsePending(
        unit_ready
            .map_owner(|()| owner)
            .begin_host_completed_packets_error(
                LeHostCompletedPacketsErrorEvent::invalid_parameters(),
            ),
    );
    assert_eq!(pending.owner().id, 41);

    let pending = pending.map_owner(|owner| owner);
    let cancelled = block_on(select(
        async {},
        pending.wait_response_capacity(&first.controller),
    ));
    assert!(matches!(cancelled, Either::First(())));
    let Publication::Pending(pending) = pending.try_publish_response(&first.controller) else {
        panic!("full output storage must return the same pending owner");
    };
    assert_eq!(pending.axis(), Axis::ResponsePending);
    assert_eq!(pending.owner().id, 41);
    assert_eq!(dropped.get(), 0);

    let mut buffer = [0; 80];
    let first_response: ControllerToHostPacket<'_> =
        block_on(first.host.read(&mut buffer)).expect("the Host drains the older response");
    assert_eq!(first_response.kind(), PacketKind::Event);
    assert_eq!(
        block_on(pending.wait_response_capacity(&first.controller)),
        Ok(ResponseWait::CapacityAvailable)
    );
    let Publication::Published(ready) = pending.try_publish_response(&first.controller) else {
        panic!("retry publishes exactly once after matching capacity returns");
    };
    assert_eq!(ready.owner().id, 41);
    assert_eq!(ready.axis(), Axis::CommandReady);
    let second_response: ControllerToHostPacket<'_> =
        block_on(first.host.read(&mut buffer)).expect("the Host receives the retried response");
    assert_eq!(second_response.kind(), PacketKind::Event);
    let no_third_response = block_on(select(
        async {},
        first.host.read::<ControllerToHostPacket<'_>>(&mut buffer),
    ));
    assert!(matches!(no_third_response, Either::First(())));
    drop(ready);
    assert_eq!(dropped.get(), 1);
}

#[test]
fn command_ready_wait_is_non_consuming_and_keeps_endpoint_affinity() {
    let dropped = Rc::new(Cell::new(0));
    let mut storage = resources();
    let mut endpoints = storage.split();
    let LeControllerCommandReadyClaim::Ready(ready) = endpoints
        .controller
        .claim_initial_command_ready(LocalOwner {
            id: 7,
            dropped: Rc::clone(&dropped),
        })
    else {
        panic!("the test claims the initial command authority");
    };
    let ready = Order::CommandReady(ready);
    assert_eq!(
        block_on(ready.wait_response_capacity(&endpoints.controller)),
        Ok(ResponseWait::CommandReady)
    );
    let Publication::CommandReady(ready) = ready.try_publish_response(&endpoints.controller) else {
        panic!("command-ready order does not manufacture a response");
    };
    assert!(ready.accepts_endpoint(&endpoints.controller));
    assert_eq!(ready.owner().id, 7);
    drop(ready);
    assert_eq!(dropped.get(), 1);
}
