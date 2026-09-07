use super::*;
use crate::{
    EnergyScanRequest, FcsStatus, FramePending, FrameView, RadioFault, ReceivedFrame, RxMetadata,
    SecurityStatus, TxRequest,
};

const ID: RequestId = RequestId::new(7);

fn channel(raw: u8) -> Channel {
    Channel::new(raw).unwrap()
}

fn metadata(channel: Channel) -> RxMetadata {
    RxMetadata {
        channel,
        rssi_dbm: -42,
        link_quality: 211,
        timestamp: None,
        fcs: FcsStatus::Valid,
        security: SecurityStatus::Unprocessed,
        frame_pending: crate::FramePending::Unavailable,
    }
}

fn enabled(capabilities: RadioCapabilities) -> RadioStateMachine {
    let mut machine = RadioStateMachine::new(capabilities);
    machine.admit(RadioCommand::Enable { id: ID }).unwrap();
    machine
}

#[test]
fn finite_enable_receive_sleep_disable_path_is_exact() {
    let mut machine = RadioStateMachine::new(RadioCapabilities::NONE);
    assert_eq!(machine.state(), RadioState::Disabled);
    let enable = machine.admit(RadioCommand::Enable { id: ID }).unwrap();
    assert_eq!(enable.previous, RadioState::Disabled);
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));

    machine
        .admit(RadioCommand::Receive {
            id: RequestId::new(8),
            channel: channel(15),
        })
        .unwrap();
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        })
    );
    machine
        .admit(RadioCommand::Sleep {
            id: RequestId::new(9),
        })
        .unwrap();
    machine
        .admit(RadioCommand::Disable {
            id: RequestId::new(10),
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Disabled);
}

#[test]
fn transmit_correlates_completion_and_restores_receive() {
    let capabilities = RadioCapabilities::CSMA_CA
        | RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT
        | RadioCapabilities::TRANSMIT_POWER;
    let mut machine = enabled(capabilities);
    machine
        .admit(RadioCommand::Receive {
            id: RequestId::new(1),
            channel: channel(20),
        })
        .unwrap();
    let bytes = [0x61, 0x88, 0x2a];
    let request = TxRequest {
        id: ID,
        frame: FrameView::new(&bytes).unwrap(),
        channel: channel(20),
        mode: TxMode::CsmaCa { max_backoffs: 4 },
        transmit_power_dbm: Some(3),
    };
    machine.admit(RadioCommand::Transmit(request)).unwrap();
    assert_eq!(
        machine.admit(RadioCommand::Sleep {
            id: RequestId::new(99)
        }),
        Err(CommandError::Busy {
            state: machine.state()
        })
    );
    assert_eq!(
        machine.observe(RadioEvent::TransmitDone {
            id: RequestId::new(6),
            status: TxStatus::Success,
            acknowledgement: None,
        }),
        Err(EventError::RequestMismatch {
            expected: ID,
            actual: RequestId::new(6),
        })
    );
    machine
        .observe(RadioEvent::TransmitDone {
            id: ID,
            status: TxStatus::Success,
            acknowledgement: None,
        })
        .unwrap();
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(20)
        })
    );
}

#[test]
fn unsupported_operations_leave_state_unchanged() {
    let mut machine = enabled(RadioCapabilities::NONE);
    let before = machine.state();
    assert_eq!(
        machine.admit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: ID,
            channel: channel(11),
            duration_us: 128,
        })),
        Err(CommandError::Unsupported {
            command: CommandKind::EnergyScan,
            required: RadioCapabilities::ENERGY_SCAN,
        })
    );
    assert_eq!(machine.state(), before);
}

#[test]
fn acknowledgement_capability_is_derived_only_from_the_fcf() {
    let no_ack_bytes = [0x01];
    let mut no_ack_machine = enabled(RadioCapabilities::NONE);
    no_ack_machine
        .admit(RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&no_ack_bytes).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
        }))
        .unwrap();

    let ack_bytes = [0x21];
    let mut ack_machine = enabled(RadioCapabilities::NONE);
    assert_eq!(
        ack_machine.admit(RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&ack_bytes).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
        })),
        Err(CommandError::Unsupported {
            command: CommandKind::Transmit,
            required: RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT,
        })
    );
}

#[test]
fn receive_and_ack_channels_are_checked() {
    let mut machine = enabled(RadioCapabilities::NONE);
    machine
        .admit(RadioCommand::Receive {
            id: ID,
            channel: channel(11),
        })
        .unwrap();
    let bytes = [0x02];
    let received = ReceivedFrame {
        frame: FrameView::new(&bytes).unwrap(),
        metadata: metadata(channel(12)),
    };
    assert_eq!(
        machine.observe(RadioEvent::Received(received)),
        Err(EventError::ChannelMismatch {
            expected: channel(11),
            actual: channel(12),
        })
    );
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(11)
        })
    );
}

#[test]
fn matching_fault_disables_and_mismatched_fault_is_rejected() {
    let capabilities = RadioCapabilities::ENERGY_SCAN;
    let mut machine = enabled(capabilities);
    machine
        .admit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: ID,
            channel: channel(26),
            duration_us: 64,
        }))
        .unwrap();
    assert_eq!(
        machine.observe(RadioEvent::Fault {
            id: None,
            fault: RadioFault::StateLost,
        }),
        Err(EventError::FaultRequestMismatch {
            expected: Some(ID),
            actual: None,
        })
    );
    machine
        .observe(RadioEvent::Fault {
            id: Some(ID),
            fault: RadioFault::StateLost,
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Disabled);
}

#[test]
fn failed_transmit_cannot_publish_an_acknowledgement() {
    let capabilities = RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT;
    let mut machine = enabled(capabilities);
    let frame_bytes = [0x21];
    machine
        .admit(RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&frame_bytes).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
        }))
        .unwrap();
    let ack_bytes = [2];
    let ack = ReceivedFrame {
        frame: FrameView::new(&ack_bytes).unwrap(),
        metadata: RxMetadata {
            frame_pending: FramePending::Clear,
            ..metadata(channel(15))
        },
    };
    assert_eq!(
        machine.observe(RadioEvent::TransmitDone {
            id: ID,
            status: TxStatus::NoAcknowledgement,
            acknowledgement: Some(ack),
        }),
        Err(EventError::AcknowledgementOnFailedTransmit)
    );
}
