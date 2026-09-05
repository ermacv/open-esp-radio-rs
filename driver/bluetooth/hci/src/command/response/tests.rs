use bt_hci::{
    ControllerToHostPacket, FromHciBytes,
    cmd::{Opcode, OpcodeGroup},
    event::{CommandComplete, CommandCompleteWithStatus},
    param::Error as HciError,
    transport::Transport,
};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use crate::{HciControllerResponse, InProcessHciChannel};

use super::UnknownCommandCompleteEvent;

#[test]
fn unknown_command_response_roundtrips_through_the_real_hci_boundary() {
    let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
    let response = UnknownCommandCompleteEvent::new(opcode);
    let mut channel = InProcessHciChannel::<NoopRawMutex, 1, 1, 16>::new();
    let (host, controller) = channel.split();

    controller
        .try_publish(response.kind(), response.as_bytes())
        .expect("the owned completion fits the empty Controller queue");

    let mut packet = [0; 16];
    let ControllerToHostPacket::Event(event) =
        block_on(host.read(&mut packet)).expect("the Host receives the retained completion")
    else {
        panic!("Unknown Command completion changed packet kind");
    };
    let complete = CommandComplete::from_hci_bytes_complete(event.data)
        .expect("the response is a complete Command Complete event");
    let complete: CommandCompleteWithStatus<'_> = complete
        .try_into()
        .expect("the response retains its status return parameter");

    assert_eq!(complete.cmd_opcode, opcode);
    assert_eq!(complete.status, HciError::UNKNOWN_CMD.to_status());
    assert!(complete.return_param_bytes.is_empty());
}
