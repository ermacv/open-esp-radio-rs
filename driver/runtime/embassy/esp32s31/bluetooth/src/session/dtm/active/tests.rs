use core::{
    future::{Future, pending, ready},
    task::{Context, Poll},
};

use bt_hci::{
    cmd::le::LeTestEnd,
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    param::ConnHandle,
    transport::Transport,
};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_bluetooth_hci::{
    BluetoothPublicDeviceAddress, HostToControllerFrame, LeControllerBootstrapConfig,
    LeControllerCommandIntake, LeControllerCommandReadyClaim, LeControllerHciResources,
    LeControllerIdleClassifiedCommandRoute,
};
use std::{boxed::Box, task::Waker};

use super::{EmbassyBluetoothDtmActiveRadioSignal, RadioFirst, select_radio_first};

type TestResources = LeControllerHciResources<NoopRawMutex, 2, 1, 45>;

fn test_resources() -> TestResources {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
        12,
        1,
    )
    .expect("the test profile has one nonzero ACL credit");
    TestResources::new(config).expect("the test profile fits its owned transport")
}

#[test]
fn radio_wins_a_simultaneous_ready_tie() {
    assert!(matches!(
        block_on(select_radio_first(ready(7_u8), ready(9_u8))),
        RadioFirst::Radio(7)
    ));
}

#[test]
fn capacity_wins_only_while_radio_is_pending() {
    assert!(matches!(
        block_on(select_radio_first(pending::<()>(), ready(9_u8))),
        RadioFirst::Other(9)
    ));
}

#[test]
fn production_readiness_keeps_command_on_radio_tie_then_sync_routes_it() {
    let mut resources = test_resources();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(command_ready) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh test epoch exposes initial command authority")
    };
    block_on(endpoints.host.write(&LeTestEnd::new())).unwrap();

    let first = block_on(select_radio_first(
        ready(EmbassyBluetoothDtmActiveRadioSignal::Scheduler),
        endpoints.controller.wait_command_available(&command_ready),
    ));
    assert!(matches!(
        first,
        RadioFirst::Radio(EmbassyBluetoothDtmActiveRadioSignal::Scheduler)
    ));

    let second = block_on(select_radio_first(
        pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
        endpoints.controller.wait_command_available(&command_ready),
    ));
    assert!(matches!(second, RadioFirst::Other(Ok(()))));

    let mut packet = [0; 45];
    let command = match endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, &mut packet)
    {
        LeControllerCommandIntake::Command { command, .. } => command,
        _ => panic!("Host readiness must leave Test End for synchronous classification"),
    };
    assert!(matches!(
        endpoints.controller.route_idle_classified_command(command),
        LeControllerIdleClassifiedCommandRoute::ResponsePending(_)
    ));
}

#[test]
fn production_readiness_leaves_exact_acl_for_synchronous_receive() {
    let mut resources = test_resources();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(command_ready) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh test epoch exposes initial command authority")
    };
    let acl = AclPacket::new(
        ConnHandle::new(1),
        AclPacketBoundary::Complete,
        AclBroadcastFlag::PointToPoint,
        &[7, 8],
    );
    block_on(endpoints.host.write(&acl)).unwrap();

    let selected = block_on(select_radio_first(
        pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
        endpoints.controller.wait_command_available(&command_ready),
    ));
    assert!(matches!(selected, RadioFirst::Other(Ok(()))));

    let mut packet = [0; 45];
    let LeControllerCommandIntake::NonCommand { frame, .. } = endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, &mut packet)
    else {
        panic!("Host readiness must leave ACL for the synchronous router");
    };
    let HostToControllerFrame::Acl(received) = frame.value() else {
        panic!("the exact Host ACL packet changed kind")
    };
    assert_eq!(received.handle(), ConnHandle::new(1));
    assert_eq!(received.boundary_flag(), AclPacketBoundary::Complete);
    assert_eq!(received.broadcast_flag(), AclBroadcastFlag::PointToPoint);
    assert_eq!(received.data(), &[7, 8]);
}

#[test]
fn cancelling_notified_production_readiness_leaves_exact_command() {
    let mut resources = test_resources();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(command_ready) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh test epoch exposes initial command authority")
    };
    let mut selected = Box::pin(select_radio_first(
        pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
        endpoints.controller.wait_command_available(&command_ready),
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        selected.as_mut().poll(&mut context),
        Poll::Pending
    ));

    block_on(endpoints.host.write(&LeTestEnd::new())).unwrap();
    drop(selected);

    assert!(matches!(
        block_on(select_radio_first(
            pending::<EmbassyBluetoothDtmActiveRadioSignal>(),
            endpoints.controller.wait_command_available(&command_ready),
        )),
        RadioFirst::Other(Ok(()))
    ));
    let mut packet = [0; 45];
    let command = match endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, &mut packet)
    {
        LeControllerCommandIntake::Command { command, .. } => command,
        _ => panic!("cancelling readiness must retain the exact queued command"),
    };
    assert!(matches!(
        endpoints.controller.route_idle_classified_command(command),
        LeControllerIdleClassifiedCommandRoute::ResponsePending(_)
    ));
}
