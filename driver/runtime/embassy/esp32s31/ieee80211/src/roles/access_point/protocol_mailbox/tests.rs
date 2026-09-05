use super::*;

#[test]
fn mailbox_preserves_typed_hardware_order_and_capacity() {
    let mut mailbox = Esp32s31AccessPointProtocolMailbox::<1>::new();
    let reset = Esp32s31AccessPointProtocolAction::Hardware(
        Esp32s31AccessPointHardwareAction::ResetRxBlockAckWindow {
            hardware_index: 2,
            tid: 6,
            starting_sequence: 0x345,
            window: 64,
        },
    );
    {
        let mut publisher = mailbox.publisher();
        publisher.try_publish(reset).unwrap();
        assert_eq!(publisher.try_publish(reset), Err(reset));
    }
    let mut receiver = mailbox.receiver();
    assert_eq!(receiver.try_receive(), Some(reset));
    assert_eq!(receiver.try_receive(), None);
}
