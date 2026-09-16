use bt_hci::{
    ControllerToHostPacket, FromHciBytes,
    cmd::{
        Cmd,
        controller_baseband::{Reset, SetEventMask},
    },
    event::{
        CommandComplete, CommandCompleteWithStatus, DisconnectionComplete, EventKind,
        ReadRemoteVersionInformationComplete, le::LeEvent,
    },
    param::{
        AddrKind, BdAddr, ClockAccuracy, ConnHandle, ControllerToHostFlowControl, Duration,
        Error as HciError, EventMask, LeAdvEventKind, LeEventMask, Status,
    },
    transport::Transport,
};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{
    LeControllerCommandReadyClaim, LeControllerHciResources, LeControllerHciResourcesError,
    LeLegacyAdvertisingReportPublication, LePeripheralConnectionEventPublication,
};
use crate::{
    BluetoothPublicDeviceAddress, BootstrapPhase, HciChannelError, LeConnectionUpdateCompleteEvent,
    LeControllerBootstrapConfig, LeControllerClassifiedCommandRoute, LeControllerCommandIntake,
    LeControllerIdleClassifiedCommandRoute, LeControllerResetCompletion,
    LeControllerResponsePublication, LeDisconnectionCompleteEvent, LeEncryptionChangeEvent,
    LeEncryptionKeyRefreshCompleteEvent, LeLegacyAdvertisingReportEvent, LeLongTermKeyRequestEvent,
    LePeripheralConnectionCompleteEvent, LeReadRemoteVersionInformationCompleteEvent,
    OwnedBootstrapCommand,
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
            required: 70,
            available: 30,
        })
    ));
    assert!(matches!(
        LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 2)),
        Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
            credits: 2,
            slots: 1,
        })
    ));
}

#[test]
fn endpoint_projects_host_acl_segmentation_and_credit_policy() {
    use core::num::NonZeroU16;

    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
        .expect("the profile fits its source-owned storage");
    {
        let endpoints = resources.split();
        let profile = endpoints.controller.controller_to_host_acl_profile();
        assert_eq!(profile.maximum_payload(), 251);
        assert_eq!(profile.total_packets(), None);
        assert!(!profile.is_flow_controlled());
    }

    assert_eq!(
        resources
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset)
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        resources
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::HostBufferSize {
                acl_data_packet_length: NonZeroU16::new(2).unwrap(),
                total_acl_data_packets: NonZeroU16::new(3).unwrap(),
            })
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        resources
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::SetControllerToHostFlowControl(
                ControllerToHostFlowControl::AclOnSyncOff,
            ))
            .status(),
        Status::SUCCESS
    );
    let endpoints = resources.split();
    let profile = endpoints.controller.controller_to_host_acl_profile();
    assert_eq!(profile.maximum_payload(), 2);
    assert_eq!(profile.total_packets(), Some(3));
    assert!(profile.is_flow_controlled());
}

#[test]
fn advertising_reports_honor_masks_and_retain_backpressure() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
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

    let mut packet = [0; 80];
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
fn peripheral_connection_events_honor_their_standard_masks() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
        .expect("the connection events fit this transport profile");
    assert_eq!(
        resources
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::Reset)
            .status(),
        Status::SUCCESS
    );
    let connection = LePeripheralConnectionCompleteEvent::new(
        ConnHandle::new(1),
        AddrKind::PUBLIC,
        BdAddr::new([1, 2, 3, 4, 5, 6]),
        Duration::from_u16(24),
        0,
        Duration::from_u16(200),
        ClockAccuracy::Ppm50,
    )
    .expect("the peer address is representable");
    let disconnection =
        LeDisconnectionCompleteEvent::new(ConnHandle::new(1), HciError::CONN_TIMEOUT.to_status());
    let update = LeConnectionUpdateCompleteEvent::new(
        ConnHandle::new(1),
        Duration::from_u16(40),
        3,
        Duration::from_u16(200),
    );
    let remote_version = LeReadRemoteVersionInformationCompleteEvent::new(
        Status::SUCCESS,
        ConnHandle::new(1),
        0x0d,
        0xffff,
        1,
    );
    let long_term_key = LeLongTermKeyRequestEvent::new(ConnHandle::new(1), [0x5a; 8], 0x1234);
    let encryption_change = LeEncryptionChangeEvent::new(Status::SUCCESS, ConnHandle::new(1), true);
    let encryption_refresh =
        LeEncryptionKeyRefreshCompleteEvent::new(Status::SUCCESS, ConnHandle::new(1));

    let endpoints = resources.split();
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::LeSetEventMask(
                LeEventMask::new().enable_le_conn_complete(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_peripheral_connection_complete(&connection),
        Ok(LePeripheralConnectionEventPublication::Masked)
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
            .try_publish_peripheral_connection_complete(&connection),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let mut packet = [0; 80];
    let ControllerToHostPacket::Event(received) =
        block_on(endpoints.host.read(&mut packet)).expect("the Host drains Connection Complete")
    else {
        panic!("Connection Complete changed packet class");
    };
    assert_eq!(received.kind, EventKind::Le);
    assert!(matches!(
        LeEvent::from_hci_bytes_complete(received.data)
            .expect("the published event remains standard"),
        LeEvent::LeConnectionComplete(_)
    ));

    assert_eq!(
        endpoints
            .controller
            .try_publish_connection_update_complete(&update),
        Ok(LePeripheralConnectionEventPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::LeSetEventMask(
                LeEventMask::new()
                    .enable_le_conn_complete(true)
                    .enable_le_conn_update_complete(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_connection_update_complete(&update),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let ControllerToHostPacket::Event(received) =
        block_on(endpoints.host.read(&mut packet)).expect("the Host drains Connection Update")
    else {
        panic!("Connection Update changed packet class");
    };
    assert!(matches!(
        LeEvent::from_hci_bytes_complete(received.data)
            .expect("the published event remains standard"),
        LeEvent::LeConnectionUpdateComplete(_)
    ));

    assert_eq!(
        endpoints
            .controller
            .try_publish_read_remote_version_information_complete(&remote_version),
        Ok(LePeripheralConnectionEventPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::SetEventMask(
                EventMask::new()
                    .enable_le_meta(true)
                    .enable_read_remote_version_information_complete(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_read_remote_version_information_complete(&remote_version),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let ControllerToHostPacket::Event(received) =
        block_on(endpoints.host.read(&mut packet)).expect("the Host drains Remote Version")
    else {
        panic!("Remote Version changed packet class");
    };
    ReadRemoteVersionInformationComplete::from_hci_bytes_complete(received.data)
        .expect("the event remains standard");

    assert_eq!(
        endpoints
            .controller
            .try_publish_long_term_key_request(&long_term_key),
        Ok(LePeripheralConnectionEventPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::LeSetEventMask(
                LeEventMask::new().enable_le_long_term_key_request(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_long_term_key_request(&long_term_key),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let ControllerToHostPacket::Event(received) =
        block_on(endpoints.host.read(&mut packet)).expect("the Host drains the LTK request")
    else {
        panic!("LTK request changed packet class");
    };
    assert!(matches!(
        LeEvent::from_hci_bytes_complete(received.data).expect("the event remains standard"),
        LeEvent::LeLongTermKeyRequest(_)
    ));

    assert_eq!(
        endpoints
            .controller
            .try_publish_encryption_change(&encryption_change),
        Ok(LePeripheralConnectionEventPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::SetEventMask(
                EventMask::new().enable_encryption_change_v1(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_encryption_change(&encryption_change),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let ControllerToHostPacket::Event(received) =
        block_on(endpoints.host.read(&mut packet)).expect("the Host drains Encryption Change")
    else {
        panic!("Encryption Change changed packet class");
    };
    assert_eq!(received.kind, EventKind::EncryptionChangeV1);

    assert_eq!(
        endpoints
            .controller
            .try_publish_encryption_key_refresh_complete(&encryption_refresh),
        Ok(LePeripheralConnectionEventPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::SetEventMask(
                EventMask::new().enable_encryption_key_refresh_complete(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_encryption_key_refresh_complete(&encryption_refresh),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let ControllerToHostPacket::Event(received) = block_on(endpoints.host.read(&mut packet))
        .expect("the Host drains Encryption Key Refresh Complete")
    else {
        panic!("Encryption Key Refresh Complete changed packet class");
    };
    assert_eq!(received.kind, EventKind::EncryptionKeyRefreshComplete);

    assert_eq!(
        endpoints
            .controller
            .try_publish_disconnection_complete(&disconnection),
        Ok(LePeripheralConnectionEventPublication::Masked)
    );
    assert_eq!(
        endpoints
            .controller
            .bootstrap
            .dispatch_owned(OwnedBootstrapCommand::SetEventMask(
                EventMask::new()
                    .enable_le_meta(true)
                    .enable_disconnection_complete(true),
            ))
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        endpoints
            .controller
            .try_publish_disconnection_complete(&disconnection),
        Ok(LePeripheralConnectionEventPublication::Published)
    );
    let ControllerToHostPacket::Event(received) =
        block_on(endpoints.host.read(&mut packet)).expect("the Host drains Disconnection Complete")
    else {
        panic!("Disconnection Complete changed packet class");
    };
    assert_eq!(received.kind, EventKind::DisconnectionComplete);
    DisconnectionComplete::from_hci_bytes_complete(received.data)
        .expect("the published event remains standard");
}

#[test]
fn one_split_exposes_host_and_the_matching_combined_command_endpoint() {
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
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
            let mut command_buffer = [0; 80];
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

            let mut event_buffer = [0; 80];
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
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
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
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
        .expect("profile fits its source-owned storage");

    {
        let mut endpoints = resources.split();
        block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the input queue");
        let mut command_buffer = [0; 80];
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
    let mut resources = LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1))
        .expect("profile fits its source-owned storage");
    let mut endpoints = resources.split();

    block_on(endpoints.host.write(&Reset::new())).expect("Reset enters the real Host transport");
    let mut reset_buffer = [0; 80];
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
    let mut command_buffer = [0; 80];
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

    let mut event_buffer = [0; 80];
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

#[test]
fn terminal_transport_close_retains_reset_response_authority_and_cannot_reopen() {
    let mut resources =
        LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1)).unwrap();
    {
        let mut endpoints = resources.split();
        block_on(endpoints.host.write(&Reset::new())).unwrap();
        let LeControllerCommandReadyClaim::Ready(initial) =
            endpoints.controller.claim_initial_command_ready(17_u32)
        else {
            panic!("fresh epoch grants authority");
        };
        let mut buffer = [0; 80];
        let LeControllerCommandIntake::Command { command, .. } = endpoints
            .controller
            .try_receive_classified_command_with_buffer(initial, &mut buffer)
        else {
            panic!("queued Reset retains its owner");
        };
        let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
            endpoints.controller.route_idle_classified_command(command)
        else {
            panic!("Reset requires quiescence");
        };
        endpoints.controller.close_transport();
        let LeControllerResetCompletion::ResponsePending(pending) = endpoints
            .controller
            .complete_reset_after_quiescence(barrier)
        else {
            panic!("same epoch still owns the accepted Reset");
        };
        // Software Reset completion cannot reopen the closed transport.
        assert_eq!(
            block_on(endpoints.host.write(&Reset::new())),
            Err(HciChannelError::Closed)
        );
        let LeControllerResponsePublication::Fault {
            pending,
            error: HciChannelError::Closed,
        } = pending.try_publish(&endpoints.controller)
        else {
            panic!("closure must retain unpublished response and affine authority");
        };
        assert_eq!(*pending.owner(), 17);
        assert!(matches!(
            endpoints.controller.claim_initial_command_ready(()),
            LeControllerCommandReadyClaim::AlreadyClaimed(())
        ));
    }
    assert!(!resources.is_pristine());
    let mut endpoints = resources.split();
    endpoints.controller.close_transport();
    assert_eq!(
        block_on(endpoints.host.write(&Reset::new())),
        Err(HciChannelError::Closed)
    );
}

#[test]
fn closing_an_unused_transport_prevents_pristine_rebinding() {
    let mut resources =
        LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 1)).unwrap();
    assert!(resources.is_pristine());
    resources.split().controller.close_transport();
    assert!(!resources.is_pristine());
}
