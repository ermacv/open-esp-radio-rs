use core::{
    future::{Future, pending},
    pin::Pin,
    task::Context,
};

use bt_hci::{
    cmd::{controller_baseband::Reset, le::LeTestEnd},
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    param::ConnHandle,
    transport::Transport,
};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_bluetooth_hci::{
    BluetoothPublicDeviceAddress, HciChannelError, HostToControllerFrame,
    LeControllerBootstrapConfig, LeControllerCommandIntake, LeControllerCommandReadyClaim,
    LeControllerHciResources, LeControllerIdleClassifiedCommandRoute,
    LeControllerResponsePublication,
};
use std::{boxed::Box, rc::Rc, task::Waker};

use super::{
    DtmSessionAction, DtmSessionStimulus, EmbassyBluetoothDtmSessionPhase, SessionOwnerSlot,
    reduce_dtm_session_transition,
};

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

#[derive(Debug, Eq, PartialEq)]
struct FakeOwner {
    generation: u8,
    drops: Rc<core::cell::Cell<u8>>,
}

impl Drop for FakeOwner {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

enum FakeSessionState {
    Active(FakeOwner),
    Quarantined {
        owner: FakeOwner,
        observation: FakeOwner,
    },
}

async fn wait_while_owner_is_stored(owner: &SessionOwnerSlot<FakeOwner>) {
    let _borrowed_owner = owner.current();
    pending::<()>().await;
}

#[test]
fn cancelling_a_borrowed_wait_retains_the_exact_owner() {
    let drops = Rc::new(core::cell::Cell::new(0));
    let slot = SessionOwnerSlot::new(FakeOwner {
        generation: 7,
        drops: Rc::clone(&drops),
    });
    let mut waiting = Box::pin(wait_while_owner_is_stored(&slot));
    let mut context = Context::from_waker(Waker::noop());
    assert!(Future::poll(Pin::as_mut(&mut waiting), &mut context).is_pending());
    drop(waiting);

    assert_eq!(slot.current().generation, 7);
    assert_eq!(drops.get(), 0);
    drop(slot);
    assert_eq!(drops.get(), 1);
}

#[test]
fn production_slot_transitions_without_copying_or_overwriting_owner() {
    let drops = Rc::new(core::cell::Cell::new(0));
    let mut slot = SessionOwnerSlot::new(FakeOwner {
        generation: 1,
        drops: Rc::clone(&drops),
    });

    let mut owner = slot.take();
    owner.generation = 2;
    let observation = slot.retain(owner, "retryable");

    assert_eq!(observation, "retryable");
    assert_eq!(slot.current().generation, 2);
    assert_eq!(drops.get(), 0);
    assert!(!slot.is_empty());
}

#[test]
fn terminal_quarantine_retains_both_affine_owners() {
    let drops = Rc::new(core::cell::Cell::new(0));
    let mut slot = SessionOwnerSlot::new(FakeSessionState::Active(FakeOwner {
        generation: 5,
        drops: Rc::clone(&drops),
    }));
    let FakeSessionState::Active(owner) = slot.take() else {
        panic!("the scripted state starts active")
    };
    slot.store(FakeSessionState::Quarantined {
        owner,
        observation: FakeOwner {
            generation: 11,
            drops: Rc::clone(&drops),
        },
    });

    let FakeSessionState::Quarantined { owner, observation } = slot.current() else {
        panic!("the unowned observation must terminate in quarantine")
    };
    assert_eq!((owner.generation, observation.generation), (5, 11));
    assert_eq!(drops.get(), 0);
    drop(slot);
    assert_eq!(drops.get(), 2);
}

#[test]
fn policy_boundary_transfers_the_owner_and_empties_the_task_slot() {
    let drops = Rc::new(core::cell::Cell::new(0));
    let mut slot = SessionOwnerSlot::new(FakeOwner {
        generation: 3,
        drops: Rc::clone(&drops),
    });

    let owner = slot.take();

    assert!(slot.is_empty());
    assert_eq!(owner.generation, 3);
    assert_eq!(drops.get(), 0);
    drop(owner);
    assert_eq!(drops.get(), 1);
}

#[test]
fn production_controller_intake_preserves_fifo_buffer_and_epoch() {
    let mut resources = test_resources();
    let mut endpoints = resources.split();
    let mut foreign_resources = test_resources();
    let foreign = foreign_resources.split();
    block_on(async {
        endpoints.host.write(&LeTestEnd::new()).await.unwrap();
        endpoints.host.write(&Reset::new()).await.unwrap();
    });
    let LeControllerCommandReadyClaim::Ready(command_ready) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh test epoch exposes initial command authority")
    };

    let mut storage = [0; 45];
    let storage_address = storage.as_mut_ptr();
    let (command_ready, buffer) = match foreign
        .controller
        .try_receive_classified_command_with_buffer(command_ready, &mut storage)
    {
        LeControllerCommandIntake::EndpointMismatch { ready, buffer } => (ready, buffer),
        _ => panic!("a foreign combined endpoint must retain authority and scratch storage"),
    };
    assert_eq!(buffer.as_mut_ptr(), storage_address);

    let (test_end, buffer) = match endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, buffer)
    {
        LeControllerCommandIntake::Command { command, buffer } => (command, buffer),
        _ => panic!("the oldest real Host command must remain routable as Test End"),
    };
    assert_eq!(buffer.as_mut_ptr(), storage_address);
    let LeControllerIdleClassifiedCommandRoute::ResponsePending(pending) =
        endpoints.controller.route_idle_classified_command(test_end)
    else {
        panic!("the oldest typed command must route as idle Test End")
    };
    let LeControllerResponsePublication::Published(command_ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("the empty response queue must return next-command authority")
    };

    let (reset, buffer) = match endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, buffer)
    {
        LeControllerCommandIntake::Command { command, buffer } => (command, buffer),
        _ => panic!("the second real Host command must remain routable as Reset"),
    };
    assert_eq!(buffer.as_mut_ptr(), storage_address);
    assert!(matches!(
        endpoints.controller.route_idle_classified_command(reset),
        LeControllerIdleClassifiedCommandRoute::ResetBarrier(_)
    ));
}

#[test]
fn production_combined_intake_transfers_exact_acl() {
    let mut resources = test_resources();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(command_ready) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh test epoch exposes initial command authority")
    };
    let acl = AclPacket::new(
        ConnHandle::new(5),
        AclPacketBoundary::Complete,
        AclBroadcastFlag::PointToPoint,
        &[11, 17],
    );
    block_on(endpoints.host.write(&acl)).unwrap();

    let mut storage = [0; 45];
    let frame = match endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, &mut storage)
    {
        LeControllerCommandIntake::NonCommand { frame, .. } => frame,
        _ => panic!("the real ACL packet must remain an external data frame"),
    };
    let HostToControllerFrame::Acl(received) = frame.value() else {
        panic!("the production classifier changed the exact ACL packet kind");
    };
    assert_eq!(received.handle(), ConnHandle::new(5));
    assert_eq!(received.boundary_flag(), AclPacketBoundary::Complete);
    assert_eq!(received.broadcast_flag(), AclBroadcastFlag::PointToPoint);
    assert_eq!(received.data(), &[11, 17]);
}

#[test]
fn production_combined_intake_preserves_the_real_channel_fault() {
    let mut resources = test_resources();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(command_ready) =
        endpoints.controller.claim_initial_command_ready(())
    else {
        panic!("the fresh test epoch exposes initial command authority")
    };
    let mut undersized = [];

    let error = match endpoints
        .controller
        .try_receive_classified_command_with_buffer(command_ready, &mut undersized)
    {
        LeControllerCommandIntake::Channel { error, buffer, .. } => {
            assert_eq!(buffer.as_mut_ptr(), undersized.as_mut_ptr());
            error
        }
        _ => panic!("an undersized production buffer must remain a channel fault"),
    };
    assert_eq!(
        error,
        HciChannelError::DestinationTooSmall {
            required: 45,
            available: 0,
        }
    );
}

#[test]
fn production_reducer_covers_each_task_phase_and_action() {
    use DtmSessionAction::{Advance, RetainBoundary, TerminalBoundary, TransferBoundary};
    use DtmSessionStimulus::{
        Completed, Continue, ControllerResponsePending, ControllerTimeExhausted, ResetBarrier,
        ResponsePublished, RestoreRequired, RetainedEndpointMismatch, RetainedExternalFrame,
        RetainedFault, Retry, StoppingResponseReady, TerminalFault, TestEnd,
        TransferredControllerEndpointMismatch, UnownedFinishedList,
    };
    use EmbassyBluetoothDtmSessionPhase::{
        CommandReady, PendingResponse, Restore, Stopping, TestEndResponse,
        UnownedFinishedList as UnownedFinishedListPhase,
    };

    let cases = [
        (PendingResponse, ResponsePublished, Advance(CommandReady)),
        (
            CommandReady,
            ControllerResponsePending,
            Advance(PendingResponse),
        ),
        (CommandReady, TestEnd, Advance(Stopping)),
        (Stopping, StoppingResponseReady, Advance(TestEndResponse)),
        (TestEndResponse, RestoreRequired, Advance(Restore)),
        (TestEndResponse, Completed, TerminalBoundary),
        (Restore, Completed, TerminalBoundary),
        (PendingResponse, Continue, Advance(PendingResponse)),
        (TestEndResponse, Continue, Advance(TestEndResponse)),
        (PendingResponse, Retry, RetainBoundary),
        (
            PendingResponse,
            UnownedFinishedList,
            Advance(UnownedFinishedListPhase),
        ),
        (
            CommandReady,
            UnownedFinishedList,
            Advance(UnownedFinishedListPhase),
        ),
        (
            Stopping,
            UnownedFinishedList,
            Advance(UnownedFinishedListPhase),
        ),
        (
            UnownedFinishedListPhase,
            UnownedFinishedList,
            RetainBoundary,
        ),
        (PendingResponse, RetainedEndpointMismatch, RetainBoundary),
        (CommandReady, RetainedFault, RetainBoundary),
        (PendingResponse, ControllerTimeExhausted, RetainBoundary),
        (CommandReady, ControllerTimeExhausted, RetainBoundary),
        (Stopping, ControllerTimeExhausted, RetainBoundary),
        (PendingResponse, TerminalFault, TerminalBoundary),
        (CommandReady, ResetBarrier, TransferBoundary),
        (
            CommandReady,
            TransferredControllerEndpointMismatch,
            TransferBoundary,
        ),
        (CommandReady, RetainedExternalFrame, RetainBoundary),
    ];

    for (phase, stimulus, expected) in cases {
        assert_eq!(reduce_dtm_session_transition(phase, stimulus), expected);
    }
}
