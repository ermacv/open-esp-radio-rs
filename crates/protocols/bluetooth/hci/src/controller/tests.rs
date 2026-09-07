use bt_hci::{
    ControllerToHostPacket, FromHciBytes,
    cmd::{
        Cmd,
        controller_baseband::{Reset, SetEventMask},
    },
    event::{CommandComplete, CommandCompleteWithStatus, EventKind},
    param::{AddrKind, BdAddr, EventMask, LeAdvEventKind, LeEventMask, Status},
    transport::Transport,
};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{
    LeControllerCommandReadyClaim, LeControllerHciResources, LeControllerHciResourcesError,
    LeLegacyAdvertisingReportPublication,
};
use crate::{
    BluetoothPublicDeviceAddress, BootstrapPhase, HciChannelError, LeControllerBootstrapConfig,
    LeControllerClassifiedCommandRoute, LeControllerCommandIntake,
    LeControllerIdleClassifiedCommandRoute, LeControllerResetCompletion,
    LeControllerResponsePublication, LeLegacyAdvertisingReportEvent, OwnedBootstrapCommand,
};

fn config(payload: u16, credits: u8) -> LeControllerBootstrapConfig {
    LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
        payload,
        credits,
    )
    .expect("nonzero test profile")
}

#[test]
fn advertised_acl_profile_must_fit_owned_storage_and_credits() {
    assert!(matches!(
        LeControllerHciResources::<NoopRawMutex, 2, 1, 30>::new(config(27, 1)),
        Err(LeControllerHciResourcesError::PacketCapacityTooSmall {
            required: 45,
            available: 30,
        })
    ));
    assert!(matches!(
        LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 2)),
        Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
            credits: 2,
            slots: 1,
        })
    ));
}

#[test]
fn advertising_reports_honor_masks_and_retain_backpressure() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
        .expect("the report event fits this transport profile");
    assert_eq!(
        resources
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset)
            .status(),
        Status::SUCCESS
    );
    let event = LeLegacyAdvertisingReportEvent::new(
        LeAdvEventKind::AdvNonconnInd,
        AddrKind::PUBLIC,
        BdAddr::new([1, 2, 3, 4, 5, 6]),
        &[2, 1, 6],
        -60,
    )
    .expect("the report is representable");

    let endpoints = resources.split();
    assert_eq!(
        endpoints
            .controller
            .try_publish_legacy_advertising_report(&event),
        Ok(LeLegacyAdvertisingReportPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::SetEventMask(
                EventMask::new().enable_le_meta(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::LeSetEventMask(
                LeEventMask::new().enable_le_adv_report(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_legacy_advertising_report(&event),
        Ok(LeLegacyAdvertisingReportPublication::Published)
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_legacy_advertising_report(&event),
        Err(HciChannelError::Full)
    );

    let mut packet = [0; 45];
    let received = block_on(endpoints.host.read(&mut packet))
        .expect("the Host drains the retained first event");
    let ControllerToHostPacket::Event(received) = received else {
        panic!("the report changed packet kind");
    };
    assert_eq!(received.kind, EventKind::Le);
    assert_eq!(
        endpoints
            .controller
            .try_publish_legacy_advertising_report(&event),
        Ok(LeLegacyAdvertisingReportPublication::Published)
    );
}

#[test]
fn one_split_exposes_host_and_the_matching_combined_command_endpoint() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
        .expect("profile fits its source-owned storage");
    assert!(resources.is_pristine());

    {
        let mut endpoints = resources.split();
        assert_eq!(endpoints.controller.bootstrap_config(), config(27, 1));
        assert_eq!(
            endpoints.controller.bootstrap_phase(),
            BootstrapPhase::AwaitingReset
        );
        block_on(async {
            endpoints
                .host
                .write(&Reset::new())
                .await
                .expect("Reset enters the bounded queue");
            let mut command_buffer = [0; 45];
            let LeControllerCommandReadyClaim::Ready(ready) =
                endpoints.controller.claim_initial_command_ready(())
            else {
                panic!("the fresh endpoint grants command authority once");
            };
            endpoints
                .controller
                .wait_command_available(&ready)
                .await
                .expect("matching authority can observe command readiness");
            let LeControllerCommandIntake::Command { command, .. } = endpoints
                .controller
                .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
            else {
                panic!("the combined endpoint consumes and classifies Reset");
            };
            let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
                endpoints.controller.route_idle_classified_command(command)
            else {
                panic!("idle Reset becomes a lifecycle barrier");
            };
            let LeControllerResetCompletion::ResponsePending(pending) = endpoints
                .controller
                .complete_reset_after_quiescence(barrier)
            else {
                panic!("the matching endpoint completes Reset after quiescence");
            };
            assert_eq!(
                endpoints.controller.bootstrap_phase(),
                BootstrapPhase::Configuring
            );
            let LeControllerResponsePublication::Published(_) =
                pending.try_publish(&endpoints.controller)
            else {
                panic!("the combined endpoint publishes the ordered completion");
            };

            let mut event_buffer = [0; 45];
            let packet = endpoints
                .host
                .read(&mut event_buffer)
                .await
                .expect("Host receives matching completion");
            assert_command_complete(packet, Reset::OPCODE, Status::SUCCESS);
        });
    }

    assert!(!resources.is_pristine());
}

#[test]
fn initial_command_ready_can_be_claimed_only_once_across_resplits() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
        .expect("profile fits its source-owned storage");

    {
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(41_u8)
        else {
            panic!("the pristine epoch exposes its sole initial authority");
        };
        assert_eq!(ready.owner(), &41);
        let LeControllerCommandReadyClaim::AlreadyClaimed(owner) =
            endpoints.controller.claim_initial_command_ready(42_u8)
        else {
            panic!("a second claim cannot mint another authority");
        };
        assert_eq!(owner, 42);
        drop(ready);
    }

    assert!(!resources.is_pristine());
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::AlreadyClaimed(owner) =
        endpoints.controller.claim_initial_command_ready(43_u8)
    else {
        panic!("dropping and resplitting cannot recreate authority");
    };
    assert_eq!(owner, 43);
}

#[test]
fn draining_a_command_cannot_reclassify_the_epoch_as_pristine() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
        .expect("profile fits its source-owned storage");

    {
        let mut endpoints = resources.split();
        block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the input queue");
        let mut command_buffer = [0; 45];
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("the fresh endpoint grants command authority once");
        };
        let LeControllerCommandIntake::Command { .. } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(ready, &mut command_buffer)
        else {
            panic!("the combined endpoint drains Reset only with command authority");
        };
    }

    assert!(!resources.is_pristine());
}

#[test]
fn combined_router_dispatches_non_reset_once_before_ordered_backpressure() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 45>::new(config(27, 1))
        .expect("profile fits its source-owned storage");
    let mut endpoints = resources.split();

    block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the real Host transport");
    let mut reset_buffer = [0; 45];
    let LeControllerCommandReadyClaim::Ready(initial) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh epoch exposes its sole initial authority");
    };
    let LeControllerCommandIntake::Command { command: reset, .. } = endpoints
        .controller
        .try_receive_classified_command_with_buffer(initial, &mut reset_buffer)
    else {
        panic!("the real endpoint classifies Reset under affine authority");
    };
    let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
        endpoints.controller.route_idle_classified_command(reset)
    else {
        panic!("idle Reset becomes a barrier before software dispatch");
    };
    let LeControllerResetCompletion::ResponsePending(prior) = endpoints
        .controller
        .complete_reset_after_quiescence(barrier)
    else {
        panic!("the matching endpoint completes the proven-idle Reset");
    };
    let LeControllerResponsePublication::Published(published) =
        prior.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue must accept the fixture Reset completion");
    };
    assert_eq!(
        endpoints.controller.bootstrap_phase(),
        BootstrapPhase::Configuring
    );

    let requested_mask = EventMask::new().enable_hardware_error(true);
    block_on(endpoints.host.write(&SetEventMask::new(requested_mask)))
        .expect("Set Event Mask enters the real Host transport");
    let mut command_buffer = [0; 45];
    let LeControllerCommandIntake::Command {
        command: classified,
        ..
    } = endpoints
        .controller
        .try_receive_classified_command_with_buffer(published, &mut command_buffer)
    else {
        panic!("the real endpoint must classify Set Event Mask under authority");
    };
    let LeControllerClassifiedCommandRoute::ResponsePending(pending) =
        endpoints.controller.route_classified_command(classified)
    else {
        panic!("non-Reset bootstrap must dispatch into the ordered response axis");
    };
    assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);

    let LeControllerResponsePublication::Pending(pending) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the queued Reset completion must backpressure Set Event Mask");
    };
    assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);

    let mut event_buffer = [0; 45];
    assert_command_complete(
        block_on(endpoints.host.read(&mut event_buffer))
            .expect("Host drains the older Reset completion"),
        Reset::OPCODE,
        Status::SUCCESS,
    );

    let LeControllerResponsePublication::Published(published) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the retained completion must publish after capacity returns");
    };
    assert_eq!(published.owner(), &());
    assert_eq!(endpoints.controller.bootstrap.event_mask(), requested_mask);
    assert_command_complete(
        block_on(endpoints.host.read(&mut event_buffer))
            .expect("Host receives the retried response"),
        SetEventMask::OPCODE,
        Status::SUCCESS,
    );
}

fn assert_command_complete(
    packet: ControllerToHostPacket<'_>,
    opcode: bt_hci::cmd::Opcode,
    status: Status,
) {
    let ControllerToHostPacket::Event(event) = packet else {
        panic!("Command Complete changed HCI packet class");
    };
    assert_eq!(event.kind, EventKind::CommandComplete);
    let complete = CommandComplete::from_hci_bytes_complete(event.data)
        .expect("event retains a complete Command Complete body");
    let complete: CommandCompleteWithStatus<'_> = complete
        .try_into()
        .expect("Command Complete retains status");
    assert_eq!(complete.cmd_opcode, opcode);
    assert_eq!(complete.status, status);
}
