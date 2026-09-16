use bt_hci::{ControllerToHostPacket, cmd::controller_baseband::Reset, transport::Transport};
use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use std::sync::{Arc, Barrier};

use super::LeControllerHciRetirementError as Error;
use crate::{
    BluetoothPublicDeviceAddress, HciChannelError, LeControllerBootstrapConfig,
    LeControllerCommandIntake, LeControllerCommandReadyClaim, LeControllerHciResources,
    LeControllerIdleClassifiedCommandRoute, LeControllerResetCompletion,
    LeControllerResponsePublication,
};

type Resources = LeControllerHciResources<NoopRawMutex, 2, 2, 80>;

fn config() -> LeControllerBootstrapConfig {
    LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
        27,
        1,
    )
    .unwrap()
}

#[test]
fn retirement_returns_owner_and_epoch_but_never_new_command_authority() {
    let mut resources = Resources::new(config()).unwrap();
    let mut endpoints = resources.split();
    let credits = endpoints.host.acl_credit_sender();
    let LeControllerCommandReadyClaim::Ready(ready) = endpoints
        .controller
        .claim_initial_command_ready(std::boxed::Box::new(73))
    else {
        panic!("initial authority");
    };
    let pointer = core::ptr::from_ref(&**ready.owner());
    let retired = endpoints
        .controller
        .try_retire_transport(ready)
        .unwrap_or_else(|_| panic!("empty queues"));
    assert!(retired.matches_endpoint(&endpoints.controller));
    let (owner, proof) = retired.into_parts();
    assert_eq!(pointer, core::ptr::from_ref(&*owner));
    assert_eq!(*owner, 73);
    assert!(proof.matches_endpoint(&endpoints.controller));
    assert!(matches!(
        endpoints.controller.claim_initial_command_ready(()),
        LeControllerCommandReadyClaim::AlreadyClaimed(())
    ));
    assert_eq!(
        block_on(endpoints.host.write(&Reset::new())),
        Err(HciChannelError::Closed)
    );
    assert_eq!(
        block_on(credits.return_completed_packets(&[])),
        Err(HciChannelError::Closed)
    );
    let mut buffer = [0; 80];
    assert!(matches!(
        block_on(
            endpoints
                .host
                .read::<ControllerToHostPacket<'_>>(&mut buffer)
        ),
        Err(HciChannelError::Closed)
    ));
    assert!(!resources.is_pristine());
}

#[test]
fn queued_reset_and_its_unread_response_each_prevent_retirement() {
    let mut resources = Resources::new(config()).unwrap();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        endpoints.controller.claim_initial_command_ready(79)
    else {
        panic!("initial authority");
    };
    block_on(endpoints.host.write(&Reset::new())).unwrap();
    let (error, ready) = endpoints
        .controller
        .try_retire_transport(ready)
        .err()
        .expect("accepted Reset must drain");
    assert_eq!(error, Error::HostPacketsPending);
    assert_eq!(*ready.owner(), 79);
    let mut buffer = [0; 80];
    let LeControllerCommandIntake::Command { command, .. } = endpoints
        .controller
        .try_receive_classified_command_with_buffer(ready, &mut buffer)
    else {
        panic!("rejection retains queued Reset and its authority");
    };
    let LeControllerIdleClassifiedCommandRoute::ResetBarrier(barrier) =
        endpoints.controller.route_idle_classified_command(command)
    else {
        panic!("Reset barrier");
    };
    let LeControllerResetCompletion::ResponsePending(pending) = endpoints
        .controller
        .complete_reset_after_quiescence(barrier)
    else {
        panic!("same endpoint");
    };
    let LeControllerResponsePublication::Published(ready) =
        pending.try_publish(&endpoints.controller)
    else {
        panic!("response capacity remains open");
    };
    let (error, ready) = endpoints
        .controller
        .try_retire_transport(ready)
        .err()
        .expect("Host must consume Reset completion");
    assert_eq!(error, Error::ControllerPacketsPending);
    let packet: ControllerToHostPacket<'_> = block_on(endpoints.host.read(&mut buffer)).unwrap();
    assert!(matches!(packet, ControllerToHostPacket::Event(_)));
    let retired = endpoints
        .controller
        .try_retire_transport(ready)
        .unwrap_or_else(|_| panic!("both queues drained"));
    assert_eq!(*retired.owner(), 79);
}

#[test]
fn rejected_retirement_keeps_independent_host_credit_admission_open() {
    let mut resources = Resources::new(config()).unwrap();
    let mut endpoints = resources.split();
    let credits = endpoints.host.acl_credit_sender();
    let LeControllerCommandReadyClaim::Ready(ready) =
        endpoints.controller.claim_initial_command_ready(83)
    else {
        panic!("initial authority");
    };
    endpoints
        .controller
        .transport()
        .try_publish(bt_hci::PacketKind::Event, &[0x10, 1, 0x42])
        .unwrap();
    let (error, ready) = endpoints
        .controller
        .try_retire_transport(ready)
        .err()
        .expect("outgoing packet retained");
    assert_eq!(error, Error::ControllerPacketsPending);
    block_on(credits.return_completed_packets(&[])).expect("credit return is still admitted");
    let mut buffer = [0; 80];
    let _: ControllerToHostPacket<'_> = block_on(endpoints.host.read(&mut buffer)).unwrap();
    let (error, ready) = endpoints
        .controller
        .try_retire_transport(ready)
        .err()
        .expect("credit command must be consumed");
    assert_eq!(error, Error::HostPacketsPending);
    assert_eq!(*ready.owner(), 83);
    assert!(
        endpoints
            .controller
            .transport()
            .try_receive(&mut buffer)
            .is_ok()
    );
    assert!(endpoints.controller.try_retire_transport(ready).is_ok());
}

#[test]
fn foreign_authority_cannot_close_either_epoch() {
    let mut first = Resources::new(config()).unwrap();
    let mut second = Resources::new(config()).unwrap();
    let mut first = first.split();
    let mut second = second.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        first.controller.claim_initial_command_ready(89)
    else {
        panic!("initial authority");
    };
    let (error, ready) = second
        .controller
        .try_retire_transport(ready)
        .err()
        .expect("foreign token");
    assert_eq!(error, Error::EndpointMismatch);
    assert_eq!(*ready.owner(), 89);
    block_on(second.host.write(&Reset::new())).unwrap();
    let retired = first
        .controller
        .try_retire_transport(ready)
        .unwrap_or_else(|_| panic!("source remains open and empty"));
    assert!(!retired.matches_endpoint(&second.controller));
}

#[test]
fn terminal_close_cannot_be_upgraded_to_graceful_retirement() {
    let mut resources = Resources::new(config()).unwrap();
    let mut endpoints = resources.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        endpoints.controller.claim_initial_command_ready(97)
    else {
        panic!("initial authority");
    };
    endpoints.controller.close_transport();
    let (error, ready) = endpoints
        .controller
        .try_retire_transport(ready)
        .err()
        .expect("terminal closure is not a graceful proof");
    assert_eq!(error, Error::Closed);
    assert_eq!(*ready.owner(), 97);
}

#[test]
fn racing_host_write_is_either_retained_or_rejected_never_lost() {
    for _ in 0..64 {
        let mut resources =
            LeControllerHciResources::<CriticalSectionRawMutex, 1, 1, 80>::new(config()).unwrap();
        let mut endpoints = resources.split();
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(101)
        else {
            panic!("initial authority");
        };
        let gate = Arc::new(Barrier::new(2));
        let (write, retirement) = std::thread::scope(|scope| {
            let writer_gate = gate.clone();
            let writer = scope.spawn(move || {
                writer_gate.wait();
                block_on(endpoints.host.write(&Reset::new()))
            });
            gate.wait();
            let retirement = endpoints.controller.try_retire_transport(ready);
            (writer.join().unwrap(), retirement)
        });
        match (write, retirement) {
            (Err(HciChannelError::Closed), Ok(retired)) => assert_eq!(*retired.owner(), 101),
            (Ok(()), Err((Error::HostPacketsPending, ready))) => {
                let mut buffer = [0; 80];
                assert!(matches!(
                    endpoints
                        .controller
                        .try_receive_classified_command_with_buffer(ready, &mut buffer),
                    LeControllerCommandIntake::Command { .. }
                ));
            }
            _ => panic!("admission and retirement must share one serialization boundary"),
        }
    }
}

#[derive(Default)]
struct DrainWake(std::sync::atomic::AtomicUsize);
impl std::task::Wake for DrainWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn drain_wait_blocks_on_unread_packet_even_when_publication_has_capacity() {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let mut resources = Resources::new(config()).unwrap();
    let endpoints = resources.split();
    endpoints
        .controller
        .transport()
        .try_publish(bt_hci::PacketKind::Event, &[0x10, 1, 0x42])
        .unwrap();
    block_on(endpoints.controller.transport().wait_publish_ready());
    let wake = Arc::new(DrainWake::default());
    let waker = Waker::from(wake.clone());
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(endpoints.controller.wait_retirement_ready());
    assert!(wait.as_mut().poll(&mut context).is_pending());
    let mut buffer = [0; 80];
    let _: ControllerToHostPacket<'_> = block_on(endpoints.host.read(&mut buffer)).unwrap();
    assert!(wake.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    // Observation alone neither closes admission nor consumes command authority.
    block_on(endpoints.host.write(&Reset::new())).unwrap();
}

#[test]
fn cancelled_drain_wait_rechecks_refilled_queues_and_does_not_steal_sender_wake() {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let mut resources = Resources::new(config()).unwrap();
    let endpoints = resources.split();
    for _ in 0..2 {
        block_on(endpoints.host.write(&Reset::new())).unwrap();
    }
    let sender_wake = Arc::new(DrainWake::default());
    let sender_waker = Waker::from(sender_wake.clone());
    let mut sender_context = Context::from_waker(&sender_waker);
    let drain_wake = Arc::new(DrainWake::default());
    let drain_waker = Waker::from(drain_wake.clone());
    let mut drain_context = Context::from_waker(&drain_waker);
    let reset = Reset::new();
    let mut sender = pin!(endpoints.host.write(&reset));
    assert!(sender.as_mut().poll(&mut sender_context).is_pending());
    {
        let mut cancelled = pin!(endpoints.controller.wait_retirement_ready());
        assert!(cancelled.as_mut().poll(&mut drain_context).is_pending());
    }
    let mut buffer = [0; 80];
    endpoints
        .controller
        .transport()
        .try_receive(&mut buffer)
        .unwrap();
    assert!(sender_wake.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
    endpoints
        .controller
        .transport()
        .try_receive(&mut buffer)
        .unwrap();
    assert!(drain_wake.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(
        sender.as_mut().poll(&mut sender_context),
        Poll::Ready(Ok(()))
    );
    let mut replacement = pin!(endpoints.controller.wait_retirement_ready());
    assert!(replacement.as_mut().poll(&mut drain_context).is_pending());
    endpoints
        .controller
        .transport()
        .try_receive(&mut buffer)
        .unwrap();
    assert_eq!(
        replacement.as_mut().poll(&mut drain_context),
        Poll::Ready(Ok(()))
    );
}

#[test]
fn drain_wait_wakes_on_close_without_turning_unread_data_into_retirement() {
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let mut resources = Resources::new(config()).unwrap();
    let mut endpoints = resources.split();
    block_on(endpoints.host.write(&Reset::new())).unwrap();
    let wake = Arc::new(DrainWake::default());
    let waker = Waker::from(wake.clone());
    let mut context = Context::from_waker(&waker);
    let mut wait = pin!(endpoints.controller.wait_retirement_ready());
    assert!(wait.as_mut().poll(&mut context).is_pending());
    endpoints.controller.close_transport();
    assert!(wake.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(
        wait.as_mut().poll(&mut context),
        Poll::Ready(Err(Error::Closed))
    );
    let mut buffer = [0; 80];
    assert!(
        endpoints
            .controller
            .transport()
            .try_receive(&mut buffer)
            .is_ok()
    );
}

#[test]
fn epoch_observation_survives_endpoint_move_and_does_not_grant_command_authority() {
    let mut resources = Resources::new(config()).unwrap();
    let endpoints = resources.split();
    let identity = endpoints.controller.epoch_identity();
    let mut controller = endpoints.controller;
    let LeControllerCommandReadyClaim::Ready(ready) = controller.claim_initial_command_ready(())
    else {
        panic!("observing identity must not consume command authority");
    };
    let proof = controller
        .try_retire_transport(ready)
        .unwrap_or_else(|_| panic!("drained"));
    assert!(proof.matches_epoch(identity));
    let mut foreign = Resources::new(config()).unwrap();
    let foreign = foreign.split();
    assert!(!proof.matches_epoch(foreign.controller.epoch_identity()));
}

#[test]
fn restart_reuses_storage_but_old_host_and_epoch_authorities_stay_closed() {
    let mut resources = Resources::new(config()).unwrap();
    let mut endpoints = resources.split();
    let old_identity = endpoints.controller.epoch_identity();
    let old_credits = endpoints.host.acl_credit_sender();
    let mut host = endpoints.host;
    for cycle in 0..16 {
        let identity = endpoints.controller.epoch_identity();
        let LeControllerCommandReadyClaim::Ready(ready) =
            endpoints.controller.claim_initial_command_ready(())
        else {
            panic!("one command authority per fresh epoch");
        };
        let retired = endpoints
            .controller
            .try_retire_transport(ready)
            .unwrap_or_else(|_| panic!("drained current epoch"));
        let new_host = endpoints
            .controller
            .restart_transport(retired)
            .unwrap_or_else(|_| panic!("exact retired epoch"));
        assert!(!identity.same_epoch(endpoints.controller.epoch_identity()));
        assert!(!old_identity.same_epoch(endpoints.controller.epoch_identity()));
        assert_eq!(
            block_on(host.write(&Reset::new())),
            Err(HciChannelError::Closed)
        );
        assert_eq!(
            block_on(old_credits.return_completed_packets(&[])),
            Err(HciChannelError::Closed)
        );
        endpoints
            .controller
            .transport()
            .try_publish(bt_hci::PacketKind::Event, &[0x10, 1, cycle])
            .unwrap();
        let mut buffer = [0; 80];
        assert!(matches!(
            block_on(host.read::<ControllerToHostPacket<'_>>(&mut buffer)),
            Err(HciChannelError::Closed)
        ));
        assert!(matches!(
            block_on(new_host.read::<ControllerToHostPacket<'_>>(&mut buffer)),
            Ok(ControllerToHostPacket::Event(_))
        ));
        assert!(matches!(
            endpoints.controller.bootstrap_phase(),
            crate::BootstrapPhase::AwaitingReset
        ));
        host = new_host;
    }
}

#[test]
fn foreign_retirement_cannot_restart_or_mutate_another_channel() {
    let mut first = Resources::new(config()).unwrap();
    let mut second = Resources::new(config()).unwrap();
    let mut first = first.split();
    let mut second = second.split();
    let LeControllerCommandReadyClaim::Ready(ready) =
        first.controller.claim_initial_command_ready(())
    else {
        panic!("ready")
    };
    let retired = first
        .controller
        .try_retire_transport(ready)
        .unwrap_or_else(|_| panic!("retired"));
    let second_identity = second.controller.epoch_identity();
    let (error, retired) = second
        .controller
        .restart_transport(retired)
        .err()
        .expect("foreign proof");
    assert_eq!(error, crate::LeControllerHciRestartError::EpochMismatch);
    assert!(second_identity.same_epoch(second.controller.epoch_identity()));
    block_on(second.host.write(&Reset::new())).unwrap();
    first
        .controller
        .restart_transport(retired)
        .unwrap_or_else(|_| panic!("original proof preserved"));
}
