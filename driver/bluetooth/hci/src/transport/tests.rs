use bt_hci::{ControllerToHostPacket, PacketKind, controller::ExternalController};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{ControllerToHostQueue, ControllerToHostQueueError, InProcessHciHostTransport};

const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

#[test]
fn queue_preserves_fifo_and_decodes_through_bt_hci() {
    let mut queue = ControllerToHostQueue::<2, 16>::new();
    queue
        .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .unwrap();
    queue.publish(PacketKind::Event, &HARDWARE_ERROR).unwrap();
    assert_eq!(queue.len(), 2);

    let mut buffer = [0; 16];
    let first = queue.receive(&mut buffer).unwrap();
    assert!(matches!(first, ControllerToHostPacket::Event(_)));
    assert_eq!(first.kind(), PacketKind::Event);
    assert_eq!(queue.front_len(), Some(HARDWARE_ERROR.len()));

    let second = queue.receive(&mut buffer).unwrap();
    assert!(matches!(second, ControllerToHostPacket::Event(_)));
    assert!(queue.is_empty());
}

#[test]
fn full_queue_never_overwrites_the_oldest_packet() {
    let mut queue = ControllerToHostQueue::<1, 16>::new();
    queue.publish(PacketKind::Event, &HARDWARE_ERROR).unwrap();

    assert_eq!(
        queue.publish(PacketKind::Event, &RESET_COMMAND_COMPLETE),
        Err(ControllerToHostQueueError::Full)
    );

    let mut buffer = [0; 16];
    let packet = queue.receive(&mut buffer).unwrap();
    let ControllerToHostPacket::Event(event) = packet else {
        panic!("the retained packet changed kind");
    };
    assert_eq!(event.data, &[0x42]);
}

#[test]
fn short_receive_buffer_retains_the_complete_oldest_packet() {
    let mut queue = ControllerToHostQueue::<1, 16>::new();
    queue
        .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .unwrap();
    let mut short = [0; 5];

    assert!(matches!(
        queue.receive(&mut short),
        Err(ControllerToHostQueueError::DestinationTooSmall {
            required: 6,
            available: 5,
        })
    ));
    assert_eq!(queue.front_len(), Some(RESET_COMMAND_COMPLETE.len()));

    let mut complete = [0; 6];
    assert!(queue.receive(&mut complete).is_ok());
    assert!(queue.is_empty());
}

#[test]
fn invalid_direction_length_and_trailing_bytes_fail_before_publication() {
    let mut queue = ControllerToHostQueue::<1, 6>::new();
    assert_eq!(
        queue.publish(PacketKind::Cmd, &[0x03, 0x0c, 0x00]),
        Err(ControllerToHostQueueError::InvalidDirection)
    );
    assert_eq!(
        queue.publish(PacketKind::Event, &[0x10, 0x00, 0xff]),
        Err(ControllerToHostQueueError::TrailingBytes)
    );
    assert_eq!(
        queue.publish(PacketKind::Event, &[0; 7]),
        Err(ControllerToHostQueueError::PacketTooLong {
            length: 7,
            capacity: 6,
        })
    );
    assert!(queue.is_empty());
}

#[test]
fn every_controller_packet_header_rejects_declared_length_mismatch() {
    let mut queue = ControllerToHostQueue::<1, 16>::new();
    for (kind, bytes) in [
        (PacketKind::AclData, &[0x01, 0x00, 0x00, 0x00, 0xaa][..]),
        (PacketKind::SyncData, &[0x01, 0x00, 0x00, 0xbb][..]),
        (PacketKind::IsoData, &[0x01, 0x00, 0x00, 0x00, 0xcc][..]),
    ] {
        assert_eq!(
            queue.publish(kind, bytes),
            Err(ControllerToHostQueueError::TrailingBytes)
        );
    }
    assert_eq!(
        queue.publish(PacketKind::AclData, &[0x01, 0x00, 0x01]),
        Err(ControllerToHostQueueError::InvalidPacket(
            bt_hci::FromHciBytesError::InvalidSize
        ))
    );
    assert!(queue.is_empty());
}

fn requires_trouble_controller<C: trouble_host::Controller>() {}

#[test]
fn bt_hci_and_trouble_share_one_controller_contract() {
    type ContractTransport = InProcessHciHostTransport<'static, NoopRawMutex, 1, 1, 16>;
    requires_trouble_controller::<ExternalController<ContractTransport, 1>>();
}
