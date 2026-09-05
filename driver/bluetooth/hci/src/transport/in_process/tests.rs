use core::{
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

use bt_hci::{
    ControllerToHostPacket, PacketKind,
    cmd::{Cmd, SyncCmd, controller_baseband::Reset, le::LeTestEnd},
    controller::{Controller, ExternalController},
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    param::ConnHandle,
    transport::Transport,
};
use embassy_futures::{
    block_on,
    join::{join, join3},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{
    HciChannelError, HciClassifiedCommandIntake, HciEpochBoundCommandReceiveError,
    HostToControllerFrame, InProcessHciChannel,
};
use crate::{LE_TEST_END_OPCODE, LeControllerCommandClassification, LeDtmCommand};

const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

#[test]
fn controller_epoch_identity_distinguishes_live_channels() {
    let mut first = TestChannel::new();
    let mut second = TestChannel::new();
    let (_, first_controller) = first.split();
    let (_, second_controller) = second.split();

    let first_identity = first_controller.epoch_identity();
    assert!(first_identity.same_epoch(first_controller.epoch_identity()));
    assert!(!first_identity.same_epoch(second_controller.epoch_identity()));
}

#[test]
fn epoch_bound_test_end_rejects_a_live_cross_wired_endpoint() {
    let mut first_channel = TestChannel::new();
    let (first_host, first_controller) = first_channel.split();
    let mut second_channel = TestChannel::new();
    let (_second_host, second_controller) = second_channel.split();

    block_on(first_host.write(&LeTestEnd::new())).expect("Test End enters its source queue");
    let mut buffer = [0; 16];
    let bound = match first_controller.try_receive_classified_command(&mut buffer) {
        Ok(bound) => bound,
        Err(_) => panic!("the source endpoint must classify its oldest Test End"),
    };
    let bound = match bound.try_into_dtm() {
        Ok(bound) => bound,
        Err(_) => panic!("Test End must retain an owned DTM command"),
    };
    let bound = match bound.try_into_test_end() {
        Ok(bound) => bound,
        Err(_) => panic!("the DTM command must retain semantic Test End ownership"),
    };

    assert!(bound.originates_from(&first_controller));
    assert!(!bound.originates_from(&second_controller));
    let bound = match bound.try_into_for_endpoint(&second_controller) {
        Ok(_) => panic!("a foreign live endpoint must not consume the semantic owner"),
        Err(bound) => bound,
    };
    let command = match bound.try_into_for_endpoint(&first_controller) {
        Ok(command) => command,
        Err(_) => panic!("the source endpoint must recover its semantic owner"),
    };
    let response = command.into_ended_command_complete(0x1234);
    assert_eq!(response.opcode(), LE_TEST_END_OPCODE);
}

#[test]
fn empty_and_cancelled_receive_create_no_epoch_token_or_consumption() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let mut buffer = [0; 16];

    assert!(matches!(
        controller.try_receive_classified_command(&mut buffer),
        Err(HciEpochBoundCommandReceiveError::Channel(
            HciChannelError::Empty
        ))
    ));

    {
        let mut cancelled = pin!(controller.receive_classified_command(&mut buffer));
        assert_pending(cancelled.as_mut());
    }

    block_on(host.write(&LeTestEnd::new()))
        .expect("the replacement receive gets one queued command");
    let bound = match block_on(controller.receive_classified_command(&mut buffer)) {
        Ok(bound) => bound,
        Err(_) => panic!("cancelled empty receive must leave the later packet available"),
    };
    assert_eq!(bound.value().opcode(), LE_TEST_END_OPCODE);
    assert!(bound.originates_from(&controller));
    assert!(matches!(
        controller.try_receive_classified_command(&mut buffer),
        Err(HciEpochBoundCommandReceiveError::Channel(
            HciChannelError::Empty
        ))
    ));
}

#[test]
fn epoch_bound_classification_preserves_host_command_fifo() {
    type FifoChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

    let mut channel = FifoChannel::new();
    let (host, controller) = channel.split();
    block_on(async {
        host.write(&LeTestEnd::new()).await.unwrap();
        host.write(&Reset::new()).await.unwrap();
    });

    let mut buffer = [0; 16];
    let first = match controller.try_receive_classified_command(&mut buffer) {
        Ok(bound) => bound,
        Err(_) => panic!("the oldest Test End must be classified first"),
    };
    assert_eq!(first.value().opcode(), LE_TEST_END_OPCODE);
    assert!(matches!(
        first.value(),
        LeControllerCommandClassification::Dtm(LeDtmCommand::TestEnd(_))
    ));

    let second = match controller.try_receive_classified_command(&mut buffer) {
        Ok(bound) => bound,
        Err(_) => panic!("Reset must remain second in the Host FIFO"),
    };
    assert_eq!(second.value().opcode(), Reset::OPCODE);
    assert!(matches!(
        second.value(),
        LeControllerCommandClassification::Bootstrap(_)
    ));
    assert!(first.originates_from(&controller));
    assert!(second.originates_from(&controller));
}

#[test]
fn publish_readiness_wait_is_side_effect_free_and_wakes_after_drain() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(controller.wait_publish_ready());
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();

    let mut wait = pin!(controller.wait_publish_ready());
    assert_pending(wait.as_mut());
    let mut event_buffer = [0; 16];
    block_on(host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)).unwrap();
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));

    controller
        .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .expect("readiness did not manufacture or reserve a packet");
}

#[test]
fn receive_readiness_wait_is_side_effect_free_and_wakes_after_publish() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    {
        let mut wait = pin!(controller.wait_receive_ready());
        assert_pending(wait.as_mut());
        block_on(host.write(&LeTestEnd::new())).unwrap();
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));
    }

    let mut buffer = [0; 16];
    let command = match controller.try_receive_classified_command(&mut buffer) {
        Ok(command) => command,
        Err(_) => panic!("readiness must leave the exact oldest packet queued"),
    };
    assert_eq!(command.value().opcode(), LE_TEST_END_OPCODE);
    assert!(matches!(
        controller.try_receive_classified_command(&mut buffer),
        Err(HciEpochBoundCommandReceiveError::Channel(
            HciChannelError::Empty
        ))
    ));
}

#[test]
fn cancelled_receive_readiness_wait_consumes_and_reserves_nothing() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    {
        let mut cancelled = pin!(controller.wait_receive_ready());
        assert_pending(cancelled.as_mut());
    }

    block_on(host.write(&LeTestEnd::new())).unwrap();
    block_on(controller.wait_receive_ready());
    block_on(controller.wait_receive_ready());

    let mut buffer = [0; 16];
    let command = match controller.try_receive_classified_command(&mut buffer) {
        Ok(command) => command,
        Err(_) => {
            panic!("cancelled and repeated readiness waits must not consume the packet")
        }
    };
    assert_eq!(command.value().opcode(), LE_TEST_END_OPCODE);
}

#[test]
fn event_loop_intake_returns_buffer_for_commands_and_stale_empty() {
    type FifoChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

    let mut channel = FifoChannel::new();
    let (host, controller) = channel.split();
    block_on(async {
        host.write(&LeTestEnd::new()).await.unwrap();
        host.write(&Reset::new()).await.unwrap();
    });

    let mut storage = [0; 16];
    let (first, buffer) = match controller.try_receive_classified_command_with_buffer(&mut storage)
    {
        HciClassifiedCommandIntake::Command { command, buffer } => (command, buffer),
        _ => panic!("the first command must return reusable storage"),
    };
    assert_eq!(first.value().opcode(), LE_TEST_END_OPCODE);

    let (second, buffer) = match controller.try_receive_classified_command_with_buffer(buffer) {
        HciClassifiedCommandIntake::Command { command, buffer } => (command, buffer),
        _ => panic!("the second command must reuse the same storage"),
    };
    assert_eq!(second.value().opcode(), Reset::OPCODE);

    let buffer = match controller.try_receive_classified_command_with_buffer(buffer) {
        HciClassifiedCommandIntake::Empty { buffer } => buffer,
        _ => panic!("stale readiness must return storage for another wait"),
    };
    assert_eq!(buffer.len(), storage.len());
}

#[test]
fn event_loop_intake_transfers_exact_data_frame_to_outer_router() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let acl = AclPacket::new(
        ConnHandle::new(7),
        AclPacketBoundary::Complete,
        AclBroadcastFlag::PointToPoint,
        &[11, 13],
    );
    block_on(host.write(&acl)).unwrap();

    let mut storage = [0; 16];
    let frame = match controller.try_receive_classified_command_with_buffer(&mut storage) {
        HciClassifiedCommandIntake::NonCommand(frame) => frame,
        _ => panic!("the data packet must transfer its exact borrowed frame"),
    };
    assert!(frame.originates_from(&controller));
    let HostToControllerFrame::Acl(received) = frame.value() else {
        panic!("the outer router must receive the original ACL kind");
    };
    assert_eq!(received.handle(), ConnHandle::new(7));
    assert_eq!(received.data(), &[11, 13]);
}

#[test]
fn cancelled_publish_readiness_wait_leaves_capacity_for_replacement() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();

    {
        let mut cancelled = pin!(controller.wait_publish_ready());
        assert_pending(cancelled.as_mut());
    }

    let mut event_buffer = [0; 16];
    block_on(host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)).unwrap();
    block_on(controller.wait_publish_ready());
    controller
        .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .expect("cancelled readiness did not consume the released slot");
}

#[test]
fn typed_reset_and_event_cross_the_direct_hci_boundary() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(async {
        host.write(&Reset::new()).await.unwrap();
        let mut command_buffer = [0; 16];
        let HostToControllerFrame::Command(command) =
            controller.receive(&mut command_buffer).await.unwrap()
        else {
            panic!("Reset changed packet kind");
        };
        assert_eq!(command.opcode(), Reset::OPCODE);
        assert!(command.parameters().is_empty());
        assert!(controller.host_to_controller.vacant_storage_is_zeroed());

        controller
            .publish(PacketKind::Event, &HARDWARE_ERROR)
            .await
            .unwrap();
        let mut event_buffer = [0; 16];
        let ControllerToHostPacket::Event(event) = host.read(&mut event_buffer).await.unwrap()
        else {
            panic!("Hardware Error changed packet kind");
        };
        assert_eq!(event.data, &[0x42]);
        assert!(controller.controller_to_host.vacant_storage_is_zeroed());
    });
}

#[test]
fn external_controller_completes_a_command_via_the_same_event_loop() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let external = ExternalController::<_, 1>::new(host);

    block_on(async {
        let reset = Reset::new();
        let mut event_buffer = external.alloc_buf().unwrap();
        let worker = async {
            let mut command_buffer = [0; 16];
            let HostToControllerFrame::Command(command) =
                controller.receive(&mut command_buffer).await.unwrap()
            else {
                panic!("Reset changed packet kind");
            };
            assert_eq!(command.opcode(), Reset::OPCODE);
            controller
                .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
                .await
                .unwrap();
            controller
                .publish(PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
        };

        let (completed, received, ()) = join3(
            reset.exec(&external),
            external.read(&mut event_buffer),
            worker,
        )
        .await;
        completed.unwrap();
        assert!(matches!(
            received.unwrap(),
            ControllerToHostPacket::Event(_)
        ));
    });
}

#[test]
fn both_async_directions_wake_without_polling_or_an_rtos() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(async {
        let mut command_buffer = [0; 16];
        let (received, sent) = join(
            controller.receive(&mut command_buffer),
            host.write(&Reset::new()),
        )
        .await;
        sent.unwrap();
        assert!(matches!(
            received.unwrap(),
            HostToControllerFrame::Command(_)
        ));

        let mut event_buffer = [0; 16];
        let (received, sent) = join(
            host.read(&mut event_buffer),
            controller.publish(PacketKind::Event, &HARDWARE_ERROR),
        )
        .await;
        sent.unwrap();
        assert!(matches!(
            received.unwrap(),
            ControllerToHostPacket::Event(_)
        ));
    });
}

#[test]
fn cancelled_backpressure_waits_never_publish_a_packet() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let reset = Reset::new();

    block_on(async {
        host.write(&reset).await.unwrap();
        {
            let mut second_write = pin!(host.write(&reset));
            assert_pending(second_write.as_mut());
        }

        let mut command_buffer = [0; 16];
        assert!(controller.receive(&mut command_buffer).await.is_ok());
        assert!(matches!(
            controller.try_receive(&mut command_buffer),
            Err(HciChannelError::Empty)
        ));

        controller
            .publish(PacketKind::Event, &HARDWARE_ERROR)
            .await
            .unwrap();
        {
            let mut second_publish =
                pin!(controller.publish(PacketKind::Event, &RESET_COMMAND_COMPLETE));
            assert_pending(second_publish.as_mut());
        }

        let mut event_buffer = [0; 16];
        assert!(
            host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)
                .await
                .is_ok()
        );
        controller
            .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
            .unwrap();
        assert!(
            host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)
                .await
                .is_ok()
        );
    });
}

#[test]
fn short_profile_buffers_fail_before_consuming_either_direction() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(async {
        host.write(&Reset::new()).await.unwrap();
        let mut short = [0; 15];
        assert!(matches!(
            controller.receive(&mut short).await,
            Err(HciChannelError::DestinationTooSmall {
                required: 16,
                available: 15,
            })
        ));
        let mut complete = [0; 16];
        assert!(controller.receive(&mut complete).await.is_ok());

        controller
            .publish(PacketKind::Event, &HARDWARE_ERROR)
            .await
            .unwrap();
        assert!(matches!(
            host.read::<ControllerToHostPacket<'_>>(&mut short).await,
            Err(HciChannelError::DestinationTooSmall {
                required: 16,
                available: 15,
            })
        ));
        assert!(
            host.read::<ControllerToHostPacket<'_>>(&mut complete)
                .await
                .is_ok()
        );
    });
}

#[test]
fn try_publication_rejects_direction_length_and_overwrite() {
    let mut channel = TestChannel::new();
    let (_host, controller) = channel.split();

    assert_eq!(
        controller.try_publish(PacketKind::Cmd, &[0x03, 0x0c, 0x00]),
        Err(HciChannelError::InvalidDirection)
    );
    assert_eq!(
        controller.try_publish(PacketKind::Event, &[0x10, 0x00, 0xff]),
        Err(HciChannelError::TrailingBytes)
    );
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();
    assert_eq!(
        controller.try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE),
        Err(HciChannelError::Full)
    );
}

#[test]
fn async_queue_preserves_fifo_across_ring_wrap() {
    type RingChannel = InProcessHciChannel<NoopRawMutex, 2, 2, 16>;
    let mut channel = RingChannel::new();
    let (host, controller) = channel.split();
    let first = [0x10, 0x01, 0x11];
    let second = [0x10, 0x01, 0x22];
    let third = [0x10, 0x01, 0x33];

    block_on(async {
        controller.publish(PacketKind::Event, &first).await.unwrap();
        controller
            .publish(PacketKind::Event, &second)
            .await
            .unwrap();
        let mut buffer = [0; 16];
        assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x11);
        controller.publish(PacketKind::Event, &third).await.unwrap();
        assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x22);
        assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x33);
        assert!(controller.controller_to_host.vacant_storage_is_zeroed());
    });
}

#[test]
fn every_host_packet_header_rejects_declared_length_mismatch() {
    for (kind, bytes) in [
        (PacketKind::Cmd, &[0x03, 0x0c, 0x00, 0xaa][..]),
        (PacketKind::AclData, &[0x01, 0x00, 0x00, 0x00, 0xaa][..]),
        (PacketKind::SyncData, &[0x01, 0x00, 0x00, 0xbb][..]),
        (PacketKind::IsoData, &[0x01, 0x00, 0x00, 0x00, 0xcc][..]),
    ] {
        assert_eq!(
            super::validate_host_packet(kind, bytes),
            Err(HciChannelError::TrailingBytes)
        );
    }
    assert_eq!(
        super::validate_host_packet(PacketKind::Cmd, &[0x03, 0x0c, 0x01]),
        Err(HciChannelError::InvalidPacket(
            bt_hci::FromHciBytesError::InvalidSize
        ))
    );
    assert_eq!(
        super::validate_host_packet(PacketKind::Event, &HARDWARE_ERROR),
        Err(HciChannelError::InvalidDirection)
    );
}

fn event_parameter(packet: ControllerToHostPacket<'_>) -> u8 {
    let ControllerToHostPacket::Event(event) = packet else {
        panic!("event changed packet kind");
    };
    event.data[0]
}

fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
}
